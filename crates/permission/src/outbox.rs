//! Empties a ref store a contained process writes, by descriptor: a host that opened these by
//! path would follow a planted `refs/heads` link and unlink a project's own branches.

use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, fstat, openat, unlinkat};

const DEPTH_MAX: usize = 8;
const ENTRIES_PER_DIR_MAX: usize = 1024;
const VALUE_MAX_BYTES: u64 = 64;

/// What one sweep took, the name relative to the store's `refs` and at most 64 bytes of value.
#[derive(Debug, Default)]
pub struct Taken {
    pub refs: Vec<(Vec<u8>, Vec<u8>)>,
    /// Names that were a link, a FIFO or anything else no ref is, and are gone now.
    pub refused: usize,
    pub past_cap: bool,
}

fn flags() -> OFlags {
    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}

fn names_in(dir: &OwnedFd) -> Vec<Vec<u8>> {
    let Ok(entries) = Dir::read_from(dir.as_fd()) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_bytes().to_vec())
        .filter(|name| name != b"." && name != b"..")
        .take(ENTRIES_PER_DIR_MAX)
        .collect()
}

/// Commit point: the unlink, before the caller acts, so a ref acts at most once. A crash after it
/// loses the request and never the work, which is in the object store.
pub fn take_refs(store: &Path, max: usize) -> Taken {
    let mut taken = Taken::default();
    let root = openat(CWD, store.join("refs"), flags(), Mode::empty());
    let Ok(root) = root else {
        return taken;
    };
    let mut walk = vec![(root, Vec::new(), 0usize)];
    while let Some((dir, prefix, depth)) = walk.pop() {
        for name in names_in(&dir) {
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
