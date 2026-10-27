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
    pub(crate) fn cached(&self, stamp: &Stamp) -> Option<Arc<T>> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.as_ref()
            .and_then(|(seen, cached)| (*seen == *stamp).then(|| Arc::clone(cached)))
    }

    pub(crate) fn store(&self, stamp: Stamp, cached: Arc<T>) -> Arc<T> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((seen, existing)) = held.as_ref()
            && *seen == stamp
        {
            return Arc::clone(existing);
        }
        *held = Some((stamp, Arc::clone(&cached)));
        cached
    }

    pub(crate) fn load(&self, stamp: Stamp, fresh: impl FnOnce() -> T) -> Arc<T> {
        if let Some(cached) = self.cached(&stamp) {
            return cached;
        }
        let cached = Arc::new(fresh());
        self.store(stamp, cached)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn stamp(mark: i64) -> Stamp {
        Stamp {
            dev: 0,
            ino: 0,
            len: 0,
            mtime: (mark, 0),
            tail: None,
        }
    }

    #[test]
    fn a_cache_hit_does_not_wait_for_a_reload() -> Result<(), Box<dyn std::error::Error>> {
        let slot = Arc::new(Slot::default());
        assert_eq!(*slot.load(stamp(1), || 1), 1);
        let (loading_tx, loading_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (hit_tx, hit_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let reloading = Arc::clone(&slot);
            scope.spawn(move || {
                reloading.load(stamp(2), || {
                    let _ = loading_tx.send(());
                    let _ = release_rx.recv();
                    2
                });
            });
            loading_rx.recv()?;
            let hitting = Arc::clone(&slot);
            scope.spawn(move || {
                let _ = hit_tx.send(hitting.load(stamp(1), || 3));
            });
            let hit = hit_rx.recv_timeout(Duration::from_millis(100));
            release_tx.send(())?;
            assert_eq!(*hit?, 1);
            Ok(())
        })
    }
}
