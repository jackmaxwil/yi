use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Invariant: inode catches replacement with length and mtime retained; a caller that must
/// also catch an in-place rewrite with restored metadata supplies the file's last line.
#[derive(PartialEq, Eq)]
pub(crate) struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    tail: Option<Vec<u8>>,
}

impl Stamp {
    pub(crate) fn of(path: &Path, tail: Option<Vec<u8>>) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            tail,
        })
    }
}

pub(crate) struct Slot<T> {
    held: Mutex<Option<(Stamp, Arc<T>)>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            held: Mutex::new(None),
        }
    }
}
impl<T> Slot<T> {
    pub(crate) fn load<E>(
        &self,
        stamp: Stamp,
        fresh: impl FnOnce() -> Result<T, E>,
    ) -> Result<Arc<T>, E> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((seen, cached)) = held.as_ref()
            && *seen == stamp
        {
            return Ok(Arc::clone(cached));
        }
        let cached = Arc::new(fresh()?);
        *held = Some((stamp, Arc::clone(&cached)));
        Ok(cached)
    }
}
