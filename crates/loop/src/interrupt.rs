use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

#[derive(Debug, Default)]
pub struct InterruptSignal {
    fired: AtomicBool,
    epoch: AtomicU64,
    wake: Notify,
    /// Invariant: a fresh flag per request, so a later request never clears an earlier one's cut.
    /// The loop (D163) or a fire sets it; the provider pump reads it between events.
    cut: Mutex<Arc<AtomicBool>>,
}

impl InterruptSignal {
    pub fn cut(&self) {
        self.cut_flag().store(true, Ordering::SeqCst);
    }

    pub fn clear_cut(&self) {
        *self.cut.lock().unwrap_or_else(PoisonError::into_inner) = Arc::default();
    }

    pub fn cut_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cut.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Invariant: the epoch moves before `fired` is set, so a reset that raced it sees the move.
    pub fn fire(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.fired.store(true, Ordering::SeqCst);
        self.cut();
        self.wake.notify_waiters();
    }

    pub fn is_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    pub fn reset_if_epoch(&self, epoch: u64) -> bool {
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return false;
        }
        self.fired.store(false, Ordering::SeqCst);
        if self.epoch.load(Ordering::SeqCst) != epoch {
            self.fired.store(true, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Invariant: a `Notified` hears `notify_waiters` from creation, so it exists before the check.
    pub async fn wait(&self) {
        let notified = self.wake.notified();
        if self.is_fired() {
            return;
        }
        notified.await;
    }
}
