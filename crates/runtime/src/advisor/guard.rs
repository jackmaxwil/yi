use std::collections::{HashSet, VecDeque};

// omp issue #3520: one session recorded 309 advise calls covering 92 unique
// notes ("Stop." ×114) — the rules must be load-bearing in code, not prose.
// The gate is invisible to the advisor model: a suppressed call still reads
// as recorded, or the model rephrases to bypass the dedupe.
const DEFAULT_HISTORY_CAPACITY: usize = 4_096;

// Conservative, normalized filler the omp reporter observed polluting the
// primary transcript; a genuine "Stop: <reason>" does not match.
const SUPPRESSED_NORMALIZED_PHRASES: [&str; 34] = [
    "stop",
    "stop here",
    "stop now",
    "halt",
    "abort",
    "done",
    "task done",
    "task complete",
    "complete",
    "finished",
    "ok",
    "okay",
    "ok done",
    "no issue",
    "no issues",
    "no issue continue",
    "no concerns",
    "no concern",
    "nothing to add",
    "nothing to flag",
    "nothing to report",
    "no notes",
    "no further input",
    "no further input needed",
    "no further input required",
    "no further watcher input",
    "no further watcher input needed",
    "no further advice",
    "no further advice needed",
    "lgtm",
    "looks good",
    "all good",
    "on track",
    "carry on",
];

/// Case-insensitive, punctuation-folded key: `"Stop."`, `"*Stop*"`, and
/// `"  stop  "` all key to `stop` (design V7, omp `normalizeAdvisorNote`).
pub fn normalize_note(note: &str) -> String {
    let mut key = String::with_capacity(note.len());
    let mut pending_space = false;
    for ch in note.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            if pending_space && !key.is_empty() {
                key.push(' ');
            }
            pending_space = false;
            key.push(ch);
        } else {
            pending_space = true;
        }
    }
    key
}

/// Design V7 (omp `AdvisorEmissionGuard`, adapted): noise filter, then
/// session-scoped dedupe (FIFO at capacity), then one accepted note per
/// review cycle. Suppressed calls never consume the per-cycle budget.
pub struct EmissionGuard {
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    consumed_this_cycle: bool,
    capacity: usize,
}

impl Default for EmissionGuard {
    fn default() -> Self {
        Self {
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            consumed_this_cycle: false,
            capacity: DEFAULT_HISTORY_CAPACITY,
        }
    }
}

impl EmissionGuard {
    pub fn begin_cycle(&mut self) {
        self.consumed_this_cycle = false;
    }

    pub fn reset(&mut self) {
        self.seen.clear();
        self.seen_order.clear();
        self.consumed_this_cycle = false;
    }

    pub fn accept(&mut self, note: &str) -> bool {
        let key = normalize_note(note);
        if key.is_empty()
            || SUPPRESSED_NORMALIZED_PHRASES.contains(&key.as_str())
            || self.seen.contains(&key)
            || self.consumed_this_cycle
        {
            return false;
        }
        self.consumed_this_cycle = true;
        self.seen.insert(key.clone());
        self.seen_order.push_back(key);
        if self.seen_order.len() > self.capacity
            && let Some(stale) = self.seen_order.pop_front()
        {
            self.seen.remove(&stale);
        }
        true
    }
}
