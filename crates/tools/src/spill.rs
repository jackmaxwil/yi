use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const HELD: usize = 64 * 1024;
const CEILING: u64 = 256 * 1024 * 1024;
/// The sweep deletes a kept spill after a week, and a `.part` file, whose writer died, after a day.
const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
const PART_FOR: Duration = Duration::from_secs(24 * 3600);

/// Every byte a producer shows the model a cut of (D316): 64 KiB held, then a `.part` file made
/// 0600 in a 0700 dir, never through a link; named `<xxh3-128>.txt` by [`Spill::keep`], or removed.
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
        static PARTS: AtomicU64 = AtomicU64::new(0);
        let dir = self.dir.clone().ok_or(io::ErrorKind::NotFound)?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&dir)?;
        let n = PARTS.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{}-{n}.part", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
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
        let (part, _) = self.part.take()?;
        sweep_hourly(&dir);
        let name = format!("{:032x}", self.hash.digest128());
        let path = dir.join(format!("{name}.txt"));
        // Invariant: a taken name is replaced only by equal bytes; a link there is never followed.
        let kept = match fs::hard_link(&part, &path) {
            Ok(()) => fs::remove_file(&part).map(|()| path),
            Err(_) if same_bytes(&path, &part) => fs::rename(&part, &path).map(|()| path),
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

/// A regular file, not a link, holding the same bytes as `part`.
fn same_bytes(path: &Path, part: &Path) -> bool {
    let regular = fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    regular && fs::read(path).ok() == fs::read(part).ok()
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

fn sweep(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let age = match path.extension().and_then(|ext| ext.to_str()) {
            Some("txt") => KEPT_FOR,
            Some("part") => PART_FOR,
            _ => continue,
        };
        let modified = entry.metadata().and_then(|meta| meta.modified());
        if modified.is_ok_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed > age)) {
            let _raced_by_another_sweep = fs::remove_file(path);
        }
    }
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
        for name in ["old.txt", ".1-1.part", "fresh.txt", "keep.me"] {
            fs::write(root.join(name), name)?;
        }
        for name in ["old.txt", ".1-1.part", "keep.me"] {
            let file = OpenOptions::new().write(true).open(root.join(name))?;
            file.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        }
        sweep(&root);
        let mut left: Vec<String> = fs::read_dir(&root)?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["fresh.txt", "keep.me"]);
        Ok(())
    }
}
