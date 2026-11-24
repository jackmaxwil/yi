use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Slot(&'static AtomicUsize);

impl Slot {
    pub(crate) fn take(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
