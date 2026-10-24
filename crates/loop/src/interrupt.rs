use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::Notify;

#[derive(Debug)]
pub struct InterruptSignal {
    fired: Arc<AtomicBool>,
    epoch: AtomicU64,
    wake: Notify,
    /// The loop's own cut of one request (D163): the provider pump reads it between events.
    cut: Arc<AtomicBool>,
}

impl Default for InterruptSignal {
    fn default() -> Self {
        Self {
            fired: Arc::new(AtomicBool::new(false)),
            epoch: AtomicU64::new(0),
            wake: Notify::new(),
            cut: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl InterruptSignal {
    pub fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
    }

    pub fn clear_cut(&self) {
        self.cut.store(false, Ordering::SeqCst);
    }

    pub fn cut_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cut)
    }

    pub fn fired_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.fired)
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
