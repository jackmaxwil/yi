use std::io::Read;
use std::path::{Path, PathBuf};

/// Incident: Chrome for Testing's linux64 headless shell is a 124 MB zip, past the 64 MiB this
/// was while every pin was a CLI tarball.
pub const MAX_ARCHIVE_BYTES: u64 = 192 * 1024 * 1024;
const MAX_TAR_BYTES: usize = 256 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 4096;
const MAX_UNZIPPED_BYTES: usize = 1024 * 1024 * 1024;

pub fn agent() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
}

pub fn read_capped(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ARCHIVE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_ARCHIVE_BYTES) {
        return Err(format!("larger than {MAX_ARCHIVE_BYTES} bytes"));
    }
    Ok(bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The digest of a file read a buffer at a time: an installed browser is a 200 MB member.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            break;
        };
        hasher.update(chunk);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// APPNOTE 6.3, read from the central directory; zip64, a link or a path leaving `into` refuses
/// it. Callers authenticate the bytes first, so CRCs are not re-checked.
pub fn unzip(zip: &[u8], into: &Path) -> Result<(), String> {
    let end = (0..zip.len().saturating_sub(21))
        .rev()
        .find(|at| zip.get(*at..at.saturating_add(4)) == Some(b"PK\x05\x06".as_slice()))
        .ok_or("is not a zip: no end record")?;
    let short_end = || "is not a zip: a short end record".to_owned();
    let count = le(zip, end.saturating_add(10), 2).ok_or_else(short_end)?;
    let start = le(zip, end.saturating_add(16), 4).ok_or_else(short_end)?;
    if count == 0xFFFF || start == 0xFFFF_FFFF {
        return Err("is zip64, which yi does not read".to_owned());
    }
    if count > MAX_ZIP_ENTRIES {
        return Err(format!(
            "has {count} entries, past the {MAX_ZIP_ENTRIES}-entry cap"
        ));
    }
    let (mut at, mut total) = (start, 0usize);
    for _ in 0..count {
        if zip.get(at..at.saturating_add(4)) != Some(b"PK\x01\x02".as_slice()) {
            return Err(format!("has no central directory row at byte {at}"));
        }
        let short = || format!("has a short central directory row at byte {at}");
        let field =
            |offset: usize, len: usize| le(zip, at.saturating_add(offset), len).ok_or_else(short);
        let (method, packed, size) = (field(10, 2)?, field(20, 4)?, field(24, 4)?);
        let (name_len, extra, comment) = (field(28, 2)?, field(30, 2)?, field(32, 2)?);
        let (mode, local) = (field(38, 4)? >> 16, field(42, 4)?);
        let name_at = at.saturating_add(46);
        let name = zip
            .get(name_at..name_at.saturating_add(name_len))
            .and_then(|name| std::str::from_utf8(name).ok())
            .ok_or_else(short)?;
        at = name_at
            .saturating_add(name_len)
            .saturating_add(extra)
            .saturating_add(comment);
        let path = name.strip_suffix('/').unwrap_or(name);
        if !stays_inside(path) {
            return Err(format!("entry {name} leaves it"));
        }
        if mode & 0o170_000 == 0o120_000 {
            return Err(format!("entry {name} is a link"));
        }
        let target = into.join(path);
        if name.ends_with('/') {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("{}: {error}", target.display()))?;
            continue;
        }
        total = total.saturating_add(size);
        if total > MAX_UNZIPPED_BYTES {
            return Err(format!("unpacks past the {MAX_UNZIPPED_BYTES}-byte cap"));
        }
        let data = (le(zip, local.saturating_add(26), 2).zip(le(zip, local.saturating_add(28), 2)))
            .map(|(name, extra)| {
                local
                    .saturating_add(30)
                    .saturating_add(name)
                    .saturating_add(extra)
            })
            .and_then(|from| zip.get(from..from.checked_add(packed)?))
            .ok_or_else(|| format!("entry {name} is truncated"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, size)
                .map_err(|error| format!("entry {name} does not inflate: {:?}", error.status))?,
            other => return Err(format!("entry {name} uses compression method {other}")),
        };
        if bytes.len() != size {
            return Err(format!(
                "entry {name} unpacks to {} bytes, not {size}",
                bytes.len()
            ));
        }
        write_file(&target, &bytes, mode & 0o111 != 0)?;
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8], executable: bool) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    std::fs::write(path, bytes).map_err(failed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(failed)?;
    }
    Ok(())
}

fn le(bytes: &[u8], at: usize, len: usize) -> Option<usize> {
    let raw = bytes.get(at..at.checked_add(len)?)?;
    Some(
        raw.iter()
            .rev()
            .fold(0, |n, byte| (n << 8) | usize::from(*byte)),
    )
}

/// RFC 1952: a 10-byte header, optional fields named by the flag byte, raw deflate, an
/// 8-byte trailer. Callers authenticate the bytes first, so the CRC is not re-checked.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let bad = || "is not gzip".to_owned();
    if bytes.get(..3) != Some(&[0x1f, 0x8b, 8][..]) {
        return Err(bad());
    }
    let flags = *bytes.get(3).ok_or_else(bad)?;
    let mut at = 10usize;
    if flags & 0x04 != 0 {
        let len = bytes.get(at..at.saturating_add(2)).ok_or_else(bad)?;
        let len = usize::from(u16::from_le_bytes(len.try_into().map_err(|_| bad())?));
        at = at.saturating_add(2).saturating_add(len);
    }
    for name_or_comment in [0x08, 0x10] {
        if flags & name_or_comment != 0 {
            let rest = bytes.get(at..).ok_or_else(bad)?;
            let end = rest.iter().position(|byte| *byte == 0).ok_or_else(bad)?;
            at = at.saturating_add(end).saturating_add(1);
        }
    }
    if flags & 0x02 != 0 {
        at = at.saturating_add(2);
    }
    let deflate = bytes
        .get(at..bytes.len().saturating_sub(8))
        .ok_or_else(bad)?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(deflate, MAX_TAR_BYTES)
        .map_err(|error| format!("does not inflate: {:?}", error.status))
}

pub struct Entry<'a> {
    pub name: String,
    pub regular: bool,
    pub bytes: &'a [u8],
}

/// Invariant: every archive member and skill file yi writes or reads by a relative path passes
/// this one check: relative, no backslash, and no empty, `.` or `..` part.
pub fn stays_inside(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// Names are returned as stored, `..` included: refusing a path is the caller's policy.
pub fn entries(mut tar: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    let mut found = Vec::new();
    while let Some((header, rest)) = tar.split_at_checked(512) {
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = entry_name(header)?;
        let size = octal(header.get(124..136).unwrap_or_default())
            .ok_or_else(|| format!("entry {name} has no size"))?;
        let padded = size
            .div_ceil(512)
            .checked_mul(512)
            .ok_or_else(|| format!("entry {name} is too large"))?;
        let bytes = rest
            .get(..size)
            .ok_or_else(|| format!("entry {name} is truncated"))?;
        let regular = matches!(header.get(156), Some(b'0' | 0));
        found.push(Entry {
            name,
            regular,
            bytes,
        });
        tar = rest.get(padded..).unwrap_or_default();
    }
    Ok(found)
}

fn entry_name(header: &[u8]) -> Result<String, String> {
    let name = header_text(header, 0, 100)?;
    if header.get(257..262) != Some(b"ustar".as_slice()) {
        return Ok(name);
    }
    let prefix = header_text(header, 345, 155)?;
    if prefix.is_empty() {
        Ok(name)
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

fn header_text(header: &[u8], at: usize, len: usize) -> Result<String, String> {
    let raw = header.get(at..at.saturating_add(len)).unwrap_or_default();
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    let bytes = raw.get(..end).unwrap_or_default();
    String::from_utf8(bytes.to_vec()).map_err(|_| "an entry name is not utf-8".to_owned())
}

fn octal(field: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(field).ok()?;
    usize::from_str_radix(text.trim_matches(['\0', ' ']), 8).ok()
}

/// The predecessor [`swap_in`] kept beside its target; [`Aside::restore`] puts it back.
#[must_use]
pub struct Aside {
    target: PathBuf,
    old: Option<PathBuf>,
    restore_clears_target: bool,
}

impl Aside {
    pub fn held(&self) -> Option<&Path> {
        self.old.as_deref()
    }

    pub fn commit(self) {
        if let Some(old) = self.old {
            let _ = std::fs::remove_dir_all(&old).or_else(|_| std::fs::remove_file(&old));
        }
    }

    /// A failed restore names the path the predecessor still sits at, so it can be moved back.
    pub fn restore(self) -> Result<(), String> {
        let Some(old) = self.old else {
            return Ok(());
        };
        if self.restore_clears_target
            && std::fs::symlink_metadata(&self.target).is_ok_and(|meta| meta.is_dir())
        {
            let _ = std::fs::remove_dir_all(&self.target);
        }
        std::fs::rename(&old, &self.target).map_err(|error| {
            let (target, old) = (self.target.display(), old.display());
            format!("{target} was not restored ({error}); the previous one is at {old}")
        })
    }
}

/// Moves `target` aside under a dot name; its restore never deletes what then sits at `target`.
pub fn set_aside(target: &Path) -> Result<Aside, String> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let old = target.with_file_name(format!(".{name}.old-{}", std::process::id()));
    rename_aside(target, old, false)
}

fn rename_aside(target: &Path, old: PathBuf, restore_clears_target: bool) -> Result<Aside, String> {
    let describe = |error: std::io::Error| format!("{}: {error}", target.display());
    if std::fs::symlink_metadata(target)
        .map_err(describe)?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "{} is a symlink; not replacing it",
            target.display()
        ));
    }
    std::fs::rename(target, &old).map_err(describe)?;
    Ok(Aside {
        target: target.to_path_buf(),
        old: Some(old),
        restore_clears_target,
    })
}

/// Invariant: a reader sees the old target or the whole new one; a file replaced by a file is one
/// rename over a hard link of it (a copy where links fail), anything else a rename aside.
pub fn swap_in(staging: &Path, target: &Path) -> Result<Aside, String> {
    swap_in_with(staging, target, |from, to| std::fs::hard_link(from, to))
}

fn swap_in_with(
    staging: &Path,
    target: &Path,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Aside, String> {
    let staged_file = std::fs::symlink_metadata(staging).is_ok_and(|meta| meta.is_file());
    let describe = |error: std::io::Error| format!("{}: {error}", target.display());
    let old = target.with_extension(format!("old-{}", std::process::id()));
    let kept = |old| Aside {
        target: target.to_path_buf(),
        old,
        restore_clears_target: true,
    };
    let aside = match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.is_dir() || !staged_file || meta.file_type().is_symlink() => {
            rename_aside(target, old, true)?
        }
        Ok(_) => match link(target, &old) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                std::fs::copy(target, &old).map(|_| ())
            }
            linked => linked,
        }
        .map(|()| kept(Some(old)))
        .map_err(describe)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => kept(None),
        Err(error) => return Err(describe(error)),
    };
    match std::fs::rename(staging, target) {
        Ok(()) => Ok(aside),
        Err(error) => Err(match aside.restore() {
            Ok(()) => describe(error),
            Err(left) => format!("{}; {left}", describe(error)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_leaving_its_directory_by_any_spelling_is_refused() {
        assert!(stays_inside("ripwire-orient/map-before-you-read.md"));
        for path in [
            "/etc/passwd",
            "a/../../b",
            "a\\..\\b",
            "./a",
            "a//b",
            "a/",
            "..",
        ] {
            assert!(!stays_inside(path), "{path}");
        }
    }

    #[test]
    fn a_failed_restore_names_where_the_old_one_is() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("tarball-restore")?;
        let (target, staging) = (scratch.join("skills"), scratch.join("skills.new"));
        std::fs::create_dir_all(&target)?;
        std::fs::write(target.join("keep"), b"old")?;
        std::fs::create_dir_all(&staging)?;
        let aside = swap_in(&staging, &target)?;
        std::fs::remove_dir_all(&target)?;
        std::fs::write(&target, b"in the way")?;
        let error = match aside.restore() {
            Ok(()) => return Err("restored a directory over a file".into()),
            Err(error) => error,
        };
        let old = target.with_extension(format!("old-{}", std::process::id()));
        assert!(error.contains(&old.display().to_string()), "{error}");
        assert_eq!(std::fs::read(old.join("keep"))?, b"old");
        Ok(())
    }

    #[test]
    fn a_file_is_replaced_by_a_directory_and_restored() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("tarball-file-to-dir")?;
        let (target, staging) = (scratch.join("python"), scratch.join("python.new"));
        std::fs::write(&target, b"stale")?;
        std::fs::create_dir_all(&staging)?;
        std::fs::write(staging.join("stamp"), b"new")?;
        let aside = swap_in(&staging, &target)?;
        assert_eq!(std::fs::read(target.join("stamp"))?, b"new");
        aside.restore()?;
        assert_eq!(std::fs::read(&target)?, b"stale");
        Ok(())
    }

    #[test]
    fn a_failed_link_falls_back_to_a_copy() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("tarball-copy")?;
        let (target, staging) = (scratch.join("yi"), scratch.join("yi.new"));
        std::fs::write(&target, b"old")?;
        std::fs::write(&staging, b"new")?;
        let cross_device = |_: &Path, _: &Path| Err(std::io::Error::other("cross-device link"));
        let aside = swap_in_with(&staging, &target, cross_device)?;
        assert_eq!(std::fs::read(&target)?, b"new");
        aside.restore()?;
        assert_eq!(std::fs::read(&target)?, b"old");
        Ok(())
    }
}
