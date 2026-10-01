use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The spill root's name under `~/.yi`, one dir per session inside it (D340).
pub const SPILLS: &str = "spills";
/// The dir beside it every session shared before spills were per session (D316).
pub const FLAT_SPILLS: &str = "tool-output";
const HELD: usize = 64 * 1024;
const CEILING: u64 = 256 * 1024 * 1024;
/// The sweep deletes a kept spill after a week, and a `.part` file, whose writer died, after a day.
const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
const PART_FOR: Duration = Duration::from_secs(24 * 3600);

/// Every byte a producer shows the model a cut of (D316): 64 KiB held, then a `.part` file 0600 in
/// one session's 0700 dir under the swept root; kept as `<xxh3-128>.txt` by [`Spill::keep`].
pub struct Spill {
    dir: Option<PathBuf>,
    held: Vec<u8>,
    part: Option<(PathBuf, File)>,
    hash: xxhash_rust::xxh3::Xxh3,
    written: u64,
    ceiling: u64,
    clipped: bool,
}

impl Spill {
    /// `None` keeps nothing.
    pub fn new(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf),
            held: Vec::new(),
            part: None,
            hash: xxhash_rust::xxh3::Xxh3::new(),
            written: 0,
            ceiling: CEILING,
            clipped: false,
        }
    }

    /// Stops at the ceiling; a failed write disables the spill, so `keep` names no partial file.
    pub fn write(&mut self, bytes: &[u8]) {
        if self.dir.is_none() {
            return;
        }
        let room = usize::try_from(self.ceiling.saturating_sub(self.written)).unwrap_or(usize::MAX);
        self.clipped |= bytes.len() > room;
        let bytes = bytes.get(..room.min(bytes.len())).unwrap_or_default();
        self.hash.update(bytes);
        let count = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.written = self.written.saturating_add(count);
        let wrote = match &mut self.part {
            Some((_, file)) => file.write_all(bytes).is_ok(),
            None => {
                self.held.extend_from_slice(bytes);
                self.held.len() <= HELD || self.open_part().is_ok()
            }
        };
        if !wrote {
            self.dir = None;
        }
    }

    fn open_part(&mut self) -> io::Result<()> {
        let dir = self.dir.clone().ok_or(io::ErrorKind::NotFound)?;
        private_dir(dir.parent().ok_or(io::ErrorKind::NotFound)?)?;
        private_dir(&dir)?;
        let path = dir.join(format!(".{}.part", random_name()?));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        self.part = Some((path, file.try_clone()?));
        file.write_all(&std::mem::take(&mut self.held))
    }

    /// The pointer to a file with every byte written, or `None` with no dir or a failed write.
    pub fn keep(&mut self) -> Option<String> {
        if self.part.is_none() && self.dir.is_some() {
            self.open_part().ok()?;
        }
        let dir = self.dir.take()?;
        let (part, file) = self.part.take()?;
        sweep_hourly(dir.parent()?);
        let name = format!("{:032x}", self.hash.digest128());
        let path = dir.join(format!("{name}.txt"));
        // Invariant: a taken name is replaced only by equal bytes; a link there is never followed.
        let kept = match fs::hard_link(&part, &path) {
            Ok(()) => fs::remove_file(&part).map(|()| path),
            Err(_) if same_bytes(&path, &file) => fs::rename(&part, &path).map(|()| path),
            Err(_) => {
                let stem = part.file_stem().unwrap_or_default().to_string_lossy();
                let own = dir.join(format!("{name}{stem}.txt"));
                fs::rename(&part, &own).map(|()| own)
            }
        };
        let path = kept.inspect_err(|_| drop(fs::remove_file(&part))).ok()?;
        let clip = (self.clipped).then(|| format!(", first {} MiB", self.ceiling >> 20));
        Some(format!(
            "[full output{}: {}]",
            clip.unwrap_or_default(),
            path.display()
        ))
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        if let Some((part, _)) = self.part.take() {
            let _gone_already_is_fine = fs::remove_file(part);
        }
    }
}

/// Created 0700, made 0700 again when open to others, and refused when it is a link: the sweep
/// would delete old files wherever a linked root led (#881).
fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    keep_private(dir)
}

/// Refused when a link or not a dir, and made 0700 again when open to group or others.
fn keep_private(dir: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        let refusal = format!("{} is a link or not a directory", dir.display());
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, refusal));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// 128 bits from the OS, so no `.part` name can be planted ahead of its writer (#881).
fn random_name() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("{:032x}", u128::from_ne_bytes(bytes)))
}

/// A regular file, not a link, holding the same bytes as `part`: opened through the read gate
/// (D339) and compared a chunk at a time, so a repeated 256 MiB output holds two chunks (#881).
fn same_bytes(path: &Path, part: &File) -> bool {
    let regular = fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    let compare = || -> io::Result<bool> {
        let context = yi_permission::CatastrophicContext::detect(path.parent().unwrap_or(path));
        let taken = yi_permission::ReadGate::new(&context).open(path, &[])?;
        if taken.metadata()?.len() != part.metadata()?.len() {
            return Ok(false);
        }
        let mut part = part;
        part.rewind()?;
        let (mut taken, mut part) = (BufReader::new(taken), BufReader::new(part));
        loop {
            let (left, right) = (taken.fill_buf()?, part.fill_buf()?);
            let n = left.len().min(right.len());
            if n == 0 || left.get(..n) != right.get(..n) {
                return Ok(left.len() == right.len() && n == 0);
            }
            taken.consume(n);
            part.consume(n);
        }
    };
    regular && compare().unwrap_or(false)
}

/// At most once an hour per process, so a long session's directory stays bounded too.
fn sweep_hourly(dir: &Path) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let Ok(mut last) = LAST.lock() else { return };
    if last.is_some_and(|at| at.elapsed() < Duration::from_secs(3600)) {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    sweep(dir);
}

/// Each session's dir under `root`, never through a link: spills older than a week, `.part`
/// files older than a day, and a dir nothing entered for a week once that empties it.
fn sweep(root: &Path) {
    if !fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir()) {
        return;
    }
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let idle = older(entry.metadata(), KEPT_FOR);
        sweep_files(&entry.path());
        if idle {
            let _written_again_or_not_empty = fs::remove_dir(entry.path());
        }
    }
    let flat = root.with_file_name(FLAT_SPILLS);
    if root.file_name().is_some_and(|name| name == SPILLS) && keep_private(&flat).is_ok() {
        sweep_files(&flat);
        let _still_holds_a_young_spill = fs::remove_dir(flat);
    }
}

fn sweep_files(dir: &Path) {
    for file in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = file.path();
        let age = match path.extension().and_then(|ext| ext.to_str()) {
            Some("txt") => KEPT_FOR,
            Some("part") => PART_FOR,
            _ => continue,
        };
        if older(file.metadata(), age) {
            let _raced_by_another_sweep = fs::remove_file(path);
        }
    }
}

fn older(meta: io::Result<fs::Metadata>, age: Duration) -> bool {
    let modified = meta.and_then(|meta| meta.modified());
    modified.is_ok_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed > age))
}

#[cfg(all(test, unix))]
mod tests {
    use super::{HELD, OpenOptions, Spill, fs, sweep};
    use crate::scratch::Scratch;

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_clipped_spill_says_where_it_stopped() -> Fallible {
        let root = Scratch::new("yi-spill-clip")?;
        let mut spill = Spill::new(Some(&root));
        spill.ceiling = 1 << 20;
        for _ in 0..17 {
            spill.write(&[b'x'; HELD]);
        }
        let note = spill.keep().ok_or("not kept")?;
        let path = (note.strip_prefix("[full output, first 1 MiB: "))
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or_else(|| note.clone())?;
        assert_eq!(fs::metadata(path)?.len(), 1 << 20);
        Ok(())
    }

    #[test]
    fn the_sweep_takes_old_spills_and_dead_parts_only() -> Fallible {
        let root = Scratch::new("yi-spill-sweep")?;
        let (live, idle) = (root.join("s1"), root.join("s2"));
        fs::create_dir_all(&live)?;
        fs::create_dir_all(&idle)?;
        for name in ["old.txt", ".1-1.part", "fresh.txt", "keep.me"] {
            fs::write(live.join(name), name)?;
        }
        for path in ["old.txt", ".1-1.part", "keep.me"].map(|name| live.join(name)) {
            let file = OpenOptions::new().write(true).open(path)?;
            file.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        }
        fs::File::open(&idle)?.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        std::os::unix::fs::symlink(&root, root.join("linked"))?;
        sweep(&root.join("linked"));
        assert!(
            live.join("old.txt").exists(),
            "the sweep went through a linked root"
        );
        let elsewhere = Scratch::new("yi-spill-sweep-elsewhere")?;
        fs::write(elsewhere.join("old.txt"), "old")?;
        let file = OpenOptions::new()
            .write(true)
            .open(elsewhere.join("old.txt"))?;
        file.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        std::os::unix::fs::symlink(&*elsewhere, root.join("s3"))?;
        sweep(&root);
        let mut left: Vec<String> = fs::read_dir(&live)?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["fresh.txt", "keep.me"]);
        assert!(!idle.exists(), "an emptied dir idle for a week stays");
        assert!(
            elsewhere.join("old.txt").exists(),
            "the sweep followed a linked session dir"
        );
        Ok(())
    }
}
