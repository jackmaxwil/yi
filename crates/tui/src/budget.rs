//! One row budget, read off the screen's height alone, sizes a live thought's window and a
//! settled tool card's result rows: no model is in the loop, and only a resize moves either.

struct Share {
    divisor: usize,
    min: usize,
    max: usize,
}

impl Share {
    fn of(&self, rows: usize) -> usize {
        let share = rows.checked_div(self.divisor).unwrap_or(0);
        share.clamp(self.min, self.max)
    }
}

const THOUGHT: Share = Share {
    divisor: 4,
    min: 4,
    max: 12,
};
const CARD: Share = Share {
    divisor: 6,
    min: 4,
    max: 12,
};
/// A failed call's result is the error the reader came for, so it gets this many shares.
const FAILED_SHARES: usize = 2;

pub fn thought_rows(rows: usize) -> usize {
    THOUGHT.of(rows)
}

pub fn card_rows(rows: usize, failed: bool) -> usize {
    let share = CARD.of(rows);
    if failed {
        share.saturating_mul(FAILED_SHARES)
    } else {
        share
    }
}

/// Of `total` result rows, those a card of `budget` shows: all of them when the cut, which spends
/// a row of its own on `… N more lines`, would save none.
pub fn shown_rows(budget: usize, total: usize) -> usize {
    if total <= budget.saturating_add(1) {
        total
    } else {
        budget
    }
}

/// Incident: a cut past the runtime's `… N more lines` row drew two counts, and a failed cut hid
/// its error tail; a cut keeps the head, one recount, then the tail, or for a pass the head alone.
pub fn cut_ends(
    keep: usize,
    total: usize,
    marker: Option<usize>,
    failed: bool,
) -> Option<(usize, usize)> {
    let after = marker.map_or(0, |at| at.saturating_add(1));
    let tail = match marker {
        _ if keep >= total => return None,
        Some(at) if at < keep => keep.saturating_sub(at).saturating_sub(1),
        _ if failed => (keep / 2).min(total.saturating_sub(after)),
        _ => return None,
    };
    Some((keep.saturating_sub(tail).saturating_sub(1), tail))
}
