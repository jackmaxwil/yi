//! A ref store a contained process writes, built and emptied by descriptor: a host that used these
//! paths would follow a planted link and write or unlink where that process chose.

use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, fstat, mkdirat, openat, unlinkat};
use rustix::io::Errno;

/// The deepest `refs/…` the sweep walks; past it an entry is named, never taken for a ref.
pub const DEPTH_MAX: usize = 8;
pub const NAMES_PER_DIR_MAX: usize = 1024;
const VALUE_MAX_BYTES: u64 = 64;

/// What one sweep took, the name relative to the store's `refs` and at most 64 bytes of value.
#[derive(Debug, Default)]
pub struct Taken {
    pub refs: Vec<(Vec<u8>, Vec<u8>)>,
    /// Names that were a link, a FIFO or anything else no ref is, and are gone now.
    pub refused: usize,
    /// A directory past [`DEPTH_MAX`]: no ref, and it stays, so a sweep says so once a call.
    pub too_deep: usize,
    pub past_cap: bool,
    pub past_names: bool,
}

fn flags() -> OFlags {
    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}

/// The store's parent is the host's own path, its one component checked `NOFOLLOW`: the admin dir
/// is writable, so `yi-outbox` itself can be a link at a project's git dir.
fn open_parent(store: &Path) -> rustix::io::Result<(OwnedFd, &std::ffi::OsStr)> {
    let (Some(parent), Some(leaf)) = (store.parent(), store.file_name()) else {
        return Err(Errno::INVAL);
    };
    let parent = openat(CWD, parent, flags() | OFlags::DIRECTORY, Mode::empty())?;
    Ok((parent, leaf))
}

fn names_in(dir: &OwnedFd) -> (Vec<Vec<u8>>, bool) {
    let Ok(entries) = Dir::read_from(dir.as_fd()) else {
        return (Vec::new(), false);
    };
    let mut names: Vec<Vec<u8>> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_bytes().to_vec())
        .filter(|name| name != b"." && name != b"..")
        .take(NAMES_PER_DIR_MAX.saturating_add(1))
        .collect();
    let past = names.len() > NAMES_PER_DIR_MAX;
    names.truncate(NAMES_PER_DIR_MAX);
    (names, past)
}

/// Commit point: the unlink, before the caller acts, so a ref acts at most once. A crash after it
/// loses the request and never the work, which is in the object store.
pub fn take_refs(store: &Path, max: usize) -> Taken {
    let mut taken = Taken::default();
    let root = open_parent(store)
        .and_then(|(parent, leaf)| openat(parent.as_fd(), leaf, flags(), Mode::empty()))
        .and_then(|store| openat(store.as_fd(), "refs", flags(), Mode::empty()));
    let root = match root {
        Ok(root) => root,
        Err(Errno::NOENT) => return taken,
        Err(_) => {
            taken.refused = 1;
            return taken;
        }
    };
    let mut walk = vec![(root, Vec::new(), 0usize)];
    while let Some((dir, prefix, depth)) = walk.pop() {
        let (names, past) = names_in(&dir);
        taken.past_names |= past;
        for name in names {
            if taken.refs.len() >= max {
                taken.past_cap = true;
                return taken;
            }
            let opened = openat(dir.as_fd(), name.as_slice(), flags(), Mode::empty());
            let kind = (opened.as_ref().ok())
                .and_then(|file| fstat(file.as_fd()).ok())
                .map(|stat| FileType::from_raw_mode(stat.st_mode));
            let path = match prefix.is_empty() {
                true => name.clone(),
                false => [prefix.as_slice(), name.as_slice()].join(&b'/'),
            };
            match (opened, kind) {
                (Ok(file), Some(FileType::Directory)) if depth < DEPTH_MAX => {
                    walk.push((file, path, depth.saturating_add(1)));
                    continue;
                }
                (Ok(_), Some(FileType::Directory)) => {
                    let _an_empty_one_goes =
                        unlinkat(dir.as_fd(), name.as_slice(), AtFlags::REMOVEDIR);
                    taken.too_deep = taken.too_deep.saturating_add(1);
                    continue;
                }
                (Ok(file), Some(FileType::RegularFile)) => {
                    let mut value = Vec::new();
                    let read = std::fs::File::from(file)
                        .take(VALUE_MAX_BYTES)
                        .read_to_end(&mut value);
                    if read.is_ok() {
                        taken.refs.push((path, value));
                    } else {
                        taken.refused = taken.refused.saturating_add(1);
                    }
                }
                _ => taken.refused = taken.refused.saturating_add(1),
            }
            let _a_directory_stays = unlinkat(dir.as_fd(), name.as_slice(), AtFlags::empty());
        }
    }
    taken
}

fn subdir(parent: &OwnedFd, name: impl AsRef<Path>) -> std::io::Result<OwnedFd> {
    let name = name.as_ref();
    let open = || {
        openat(
            parent.as_fd(),
            name,
            flags() | OFlags::DIRECTORY,
            Mode::empty(),
        )
    };
    match open() {
        Ok(dir) => return Ok(dir),
        Err(Errno::NOENT) => {}
        Err(_) => unlinkat(parent.as_fd(), name, AtFlags::empty())?,
    }
    mkdirat(parent.as_fd(), name, Mode::from_raw_mode(0o755))?;
    Ok(open()?)
}

/// Invariant: unlinked, then created exclusively, so no inode that was already there is written:
/// not through a link, a FIFO, or a hard link to a file this process could not otherwise reach.
fn fresh_file(dir: &OwnedFd, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    match unlinkat(dir.as_fd(), name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let created = OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = openat(dir.as_fd(), name, created, Mode::from_raw_mode(0o644))?;
    std::fs::File::from(file).write_all(bytes)
}

/// The bare store a contained `git push` lands in. Git never runs in it on the host, so it is laid
/// out by hand, on every start, over whatever a contained process left.
pub fn init_store(store: &Path, objects: &Path) -> std::io::Result<()> {
    let (parent, leaf) = open_parent(store)?;
    let root = subdir(&parent, leaf)?;
    let refs = subdir(&root, "refs")?;
    subdir(&refs, "heads")?;
    subdir(&refs, "tags")?;
    let info = subdir(&subdir(&root, "objects")?, "info")?;
    fresh_file(&root, "HEAD", b"ref: refs/heads/main\n")?;
    fresh_file(&root, "config", b"[core]\n\tbare = true\n")?;
    let alternates = format!("{}\n", objects.display());
    fresh_file(&info, "alternates", alternates.as_bytes())
}
