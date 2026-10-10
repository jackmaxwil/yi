use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::redact::Redactor;

const HELD: usize = 64 * 1024;
const CEILING: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Stream {
    Out,
    Err,
}

/// Every byte a producer shows the model a cut of (D316), secrets redacted: 64 KiB held, then a
/// `.part` file 0600 in a 0700 dir; kept as `<xxh3-128>.txt` by [`Spill::keep`].
pub struct Spill {
    dir: Option<PathBuf>,
    /// Invariant: one per stream, since a line stdout leaves open is not closed by stderr's.
    redactors: Option<[Redactor; 2]>,
    held: Vec<u8>,
    part: Option<(PathBuf, File)>,
    hash: xxhash_rust::xxh3::Xxh3,
    written: u64,
    ceiling: u64,
    clipped: bool,
}

impl Spill {
    /// `None` keeps nothing, and so does a redactor that cannot build.
    pub fn new(dir: Option<&Path>) -> Self {
        let redactors = Redactor::new()
            .zip(Redactor::new())
            .map(<[Redactor; 2]>::from);
        Self {
            dir: dir.filter(|_| redactors.is_some()).map(Path::to_path_buf),
            redactors,
            held: Vec::new(),
            part: None,
            hash: xxhash_rust::xxh3::Xxh3::new(),
            written: 0,
            ceiling: CEILING,
            clipped: false,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.write_to(Stream::Out, bytes);
    }

    pub fn write_to(&mut self, stream: Stream, bytes: &[u8]) {
        let (Some([out, err]), Some(_)) = (&mut self.redactors, &self.dir) else {
            return;
        };
        let redactor = match stream {
            Stream::Out => out,
            Stream::Err => err,
        };
        let mut clean = Vec::with_capacity(bytes.len());
        redactor.push(bytes, &mut clean);
        self.store(&clean);
    }

    /// Stops at the ceiling; a failed write disables the spill, so `keep` names no partial file.
    fn store(&mut self, bytes: &[u8]) {
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
        for redactor in self.redactors.take().into_iter().flatten() {
            let mut tail = Vec::new();
            redactor.finish(&mut tail);
            self.store(&tail);
        }
        if self.part.is_none() && self.dir.is_some() {
            self.open_part().ok()?;
        }
        let dir = self.dir.take()?;
        let (part, file) = self.part.take()?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::{HELD, Spill, fs};
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

    /// A key printed by a tool reaches every model that reads the kept file, so it is cut
    /// before the file is written; one line per line, so `#L` pointers still land.
    #[test]
    fn a_kept_spill_holds_no_planted_secret_and_keeps_its_lines() -> Fallible {
        let root = Scratch::new("yi-spill-redact")?;
        // Assembled, so the public mirror's scan of the tree finds no key-shaped literal.
        let token = format!("sk-{}-{}", "or-v1", "0123456789abcdef".repeat(2));
        let fence = |end: &str| format!("-----{end} OPENSSH PRIVATE KEY-----");
        let body = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAE\nbm9uZQAAAAAAAAABAAABlwAAAAdzc2gtcn";
        let key = format!("{}\n{body}\n{}\n", fence("BEGIN"), fence("END"));
        let source = format!("log line one\nexport OPENROUTER_API_KEY={token}\n{key}tail");
        let mut spill = Spill::new(Some(&root));
        let (head, rest) = source.split_at(30);
        spill.write(head.as_bytes());
        spill.write(rest.as_bytes());
        let note = spill.keep().ok_or("not kept")?;
        let path = (note.strip_prefix("[full output: "))
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or_else(|| note.clone())?;
        let kept = fs::read_to_string(path)?;
        assert!(!kept.contains("0123456789abcdef"), "{kept}");
        assert!(!kept.contains("b3BlbnNzaC1rZXkt"), "{kept}");
        assert_eq!(kept.lines().count(), source.lines().count(), "{kept}");
        assert!(kept.starts_with("log line one\n"), "{kept}");
        assert!(
            kept.ends_with("-----END OPENSSH PRIVATE KEY-----\ntail"),
            "{kept}"
        );
        Ok(())
    }
}
