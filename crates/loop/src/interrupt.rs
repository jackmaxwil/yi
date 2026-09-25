use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::Notify;

#[derive(Debug, Default)]
pub struct InterruptSignal {
    fired: AtomicBool,
    epoch: AtomicU64,
    wake: Notify,
    /// The loop's own cut of one request (D163): the provider pump reads it between events.
    cut: Arc<AtomicBool>,
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

    pub fn fire(&self) {
        self.fired.store(true, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    pub fn is_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    pub fn reset_if_epoch(&self, epoch: u64) -> bool {
        if self.epoch.load(Ordering::SeqCst) == epoch {
            self.fired.store(false, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub async fn wait(&self) {
        if self.is_fired() {
            return;
        }
        self.wake.notified().await;
    }
}
