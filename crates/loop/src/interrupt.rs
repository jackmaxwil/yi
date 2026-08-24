use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::Notify;
use yi_types::message::AgentMessage;

#[derive(Debug, Default)]
pub struct InterruptSignal {
    fired: AtomicBool,
    epoch: AtomicU64,
    wake: Notify,
}

impl InterruptSignal {
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

#[derive(Debug, Clone, PartialEq)]
pub struct SoftInterrupt {
    pub text: String,
    pub source: String,
    pub urgent: bool,
}

#[derive(Debug, Default)]
pub struct SoftInterruptQueue {
    items: Mutex<Vec<SoftInterrupt>>,
}

impl SoftInterruptQueue {
    pub fn push(&self, item: SoftInterrupt) {
        if let Ok(mut items) = self.items.lock() {
            items.push(item);
        }
    }

    pub fn drain(&self) -> Vec<SoftInterrupt> {
        match self.items.lock() {
            Ok(mut items) => std::mem::take(&mut *items),
            Err(_) => Vec::new(),
        }
    }
}

pub fn synthesize_skipped(skipped: &[(String, String)], timestamp: u64) -> Vec<AgentMessage> {
    skipped
        .iter()
        .map(|(tool_call_id, tool_name)| AgentMessage::ToolResult {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            content: vec![yi_types::message::Content::Text {
                text: "[Skipped: user interrupted]".to_owned(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: true,
            timestamp,
        })
        .collect()
}
