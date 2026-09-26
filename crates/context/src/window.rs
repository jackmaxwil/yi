use yi_types::compaction::CompactionWindow;

use crate::account::Tokens;

/// Absolute input-token baseline for the current compaction window (design §4.4
/// BodyAfterPrefix). Server-observed usage replaces an estimate but never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefill {
    ServerObserved(Tokens),
    Estimated(Tokens),
}

/// Design §4.4 window chain: ids chain compactions (surfaced to the model) and
/// per-window one-shot latches kill repeat advisories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    number: u64,
    first_id: String,
    previous_id: Option<String>,
    id: String,
    prefill: Option<Prefill>,
}

impl Window {
    pub fn new_initial(id: String) -> Self {
        Self {
            number: 0,
            first_id: id.clone(),
            previous_id: None,
            id,
            prefill: None,
        }
    }

    pub fn restore(number: u64, ids: CompactionWindow) -> Self {
        Self {
            number,
            first_id: ids.first,
            previous_id: ids.previous,
            id: ids.id,
            prefill: None,
        }
    }

    pub fn advance(&mut self, new_id: String) -> CompactionWindow {
        self.number = self.number.saturating_add(1);
        self.previous_id = Some(std::mem::replace(&mut self.id, new_id));
        self.prefill = None;
        self.ids()
    }

    pub fn ids(&self) -> CompactionWindow {
        CompactionWindow {
            first: self.first_id.clone(),
            previous: self.previous_id.clone(),
            id: self.id.clone(),
            number: self.number,
        }
    }

    pub fn observe_prefill(&mut self, observed: Prefill) {
        match (&self.prefill, &observed) {
            (Some(Prefill::ServerObserved(_)), Prefill::Estimated(_)) => {}
            _ => self.prefill = Some(observed),
        }
    }

    pub fn prefill_tokens(&self) -> Option<Tokens> {
        self.prefill.map(|prefill| match prefill {
            Prefill::ServerObserved(tokens) | Prefill::Estimated(tokens) => tokens,
        })
    }
}
