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
