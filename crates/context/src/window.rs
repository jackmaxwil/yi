use yi_types::compaction::CompactionWindow;

/// Design §4.4 window chain: ids chain compactions (surfaced to the model) and
/// per-window one-shot latches kill repeat advisories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    number: u64,
    first_id: String,
    previous_id: Option<String>,
    id: String,
}

impl Window {
    pub fn new_initial(id: String) -> Self {
        Self {
            number: 0,
            first_id: id.clone(),
            previous_id: None,
            id,
        }
    }

    pub fn restore(number: u64, ids: CompactionWindow) -> Self {
        Self {
            number,
            first_id: ids.first,
            previous_id: ids.previous,
            id: ids.id,
        }
    }

    pub fn advance(&mut self, new_id: String) -> CompactionWindow {
        self.number = self.number.saturating_add(1);
        self.previous_id = Some(std::mem::replace(&mut self.id, new_id));
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
}
