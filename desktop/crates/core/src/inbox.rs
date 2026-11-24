//! The one way into the UI thread: worker threads send messages, the UI drains them through
//! `step` in batches. The channel is bounded, so a flooding source waits for the UI.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use crate::feed::FeedEvent;
use crate::ids::FeedId;
use crate::model::Model;
use crate::msg::{Effect, Msg, step};

/// Messages the channel holds before a sender waits: the server's own per-client queue size.
pub const BOUND: usize = 4096;
/// Messages one drain steps before it yields the frame.
pub const BATCH: usize = 1024;

/// The UI side is gone: the sender's thread should end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the app's inbox is closed")]
pub struct Closed;

/// A worker thread's way to the model. Cloned freely; sending wakes the UI.
#[derive(Clone)]
pub struct Sender {
    tx: SyncSender<Msg>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sender")
    }
}

impl Sender {
    /// Queues `msg`, waiting while the channel is full. Never call it on the UI thread.
    pub fn send(&self, msg: Msg) -> Result<(), Closed> {
        self.tx.send(msg).map_err(|_| Closed)?;
        (self.wake)();
        Ok(())
    }

    /// This sender narrowed to one feed, for the source that fills it.
    pub fn feed(&self, feed: FeedId) -> FeedTx {
        FeedTx {
            feed,
            sender: self.clone(),
        }
    }
}

/// What a source holds: it can send events of its own feed and nothing else.
#[derive(Debug, Clone)]
pub struct FeedTx {
    feed: FeedId,
    sender: Sender,
}

impl FeedTx {
    pub fn id(&self) -> FeedId {
        self.feed
    }

    pub fn send(&self, event: FeedEvent) -> Result<(), Closed> {
        self.sender.send(Msg::Feed(self.feed, event))
    }
}

/// The UI thread's end of the channel.
pub struct Inbox {
    rx: Receiver<Msg>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// What one drain did.
#[derive(Debug, Default, PartialEq)]
pub struct Drained {
    pub steps: usize,
    pub effects: Vec<Effect>,
}

impl Inbox {
    /// Steps up to [`BATCH`] waiting messages into `model`; asks for another wake if more wait.
    pub fn drain(&self, model: &mut Model) -> Drained {
        let mut drained = Drained::default();
        for msg in self.rx.try_iter().take(BATCH) {
            drained.effects.extend(step(model, msg));
            drained.steps = drained.steps.saturating_add(1);
        }
        if drained.steps == BATCH {
            (self.wake)();
        }
        drained
    }
}

/// A bounded channel into the UI thread. `wake` is called after every send, from any thread.
pub fn inbox(wake: impl Fn() + Send + Sync + 'static) -> (Sender, Inbox) {
    let (tx, rx) = sync_channel(BOUND);
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    (
        Sender {
            tx,
            wake: wake.clone(),
        },
        Inbox { rx, wake },
    )
}
