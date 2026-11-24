//! A list's own state: the open row, the cursor, whether it stands at its tail.

use std::collections::BTreeMap;

use yd_core::ids::RowId;

/// The row open in place, and whether closing it sends the list back to its tail.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    row: RowId,
    resume: bool,
}

/// The selected row. `slot` is where it stood when it was chosen: a hint, checked by id.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    row: RowId,
    slot: usize,
}

/// What one list remembers between frames. It lives in the hub, so it travels with its pane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListState {
    open: Option<Open>,
    cursor: Option<Cursor>,
    at_tail: bool,
    /// What the open row's last pressed button came to, when it did nothing.
    said: Option<String>,
    /// The columns the pane's width had room for, a bit each; none until it was laid out.
    fit: Option<u32>,
}

impl ListState {
    /// The columns kept for the width, as [`crate::list::fit::kept`] chose them.
    pub fn fit(&self) -> Option<u32> {
        self.fit
    }

    pub(super) fn set_fit(&mut self, fit: u32) {
        self.fit = Some(fit);
    }

    /// The one row open in place.
    pub fn open(&self) -> Option<&RowId> {
        self.open.as_ref().map(|open| &open.row)
    }

    /// The selected row: the one the arrow keys stand on.
    pub fn cursor(&self) -> Option<&RowId> {
        self.cursor.as_ref().map(|cursor| &cursor.row)
    }

    /// Whether a list that follows its tail had its newest row in view when it last drew: a
    /// pane says "following" or "paused" from it. A list that does not follow never has.
    pub fn at_tail(&self) -> bool {
        self.at_tail
    }

    /// What the open row's buttons last said, such as `Pause is not available yet`.
    pub fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }

    /// A button of the open row says `said`, or takes back what the last one said.
    pub(super) fn say(&mut self, said: Option<String>) {
        self.said = said;
    }

    /// Opens `row` in place, or closes whatever is open: how a scene shows a row open.
    pub fn set_open(&mut self, row: Option<RowId>) {
        match row {
            Some(row) => self.show(&row),
            None => (self.open, self.said) = (None, None),
        }
    }

    /// Where the cursor stood when it was chosen, for finding it again without a search.
    pub(super) fn cursor_slot(&self) -> Option<usize> {
        self.cursor.as_ref().map(|cursor| cursor.slot)
    }

    /// A press on a row: it becomes the selected row, and opens unless it was the open one.
    /// True when the list should go back to its tail.
    pub(super) fn press(&mut self, row: &RowId, slot: usize) -> bool {
        self.select(row, slot);
        if self.open() == Some(row) {
            return self.close().unwrap_or(false);
        }
        self.show(row);
        false
    }

    /// Return on the selected row: it opens, or closes if it is the open one. `Ok` is the
    /// row now open; `Err` is [`ListState::close`]'s answer. None with no selection.
    pub(super) fn toggle_cursor(&mut self) -> Option<Result<RowId, bool>> {
        let row = self.cursor()?.clone();
        if self.open() == Some(&row) {
            return self.close().map(Err);
        }
        self.show(&row);
        Some(Ok(row))
    }

    /// Closes the open row. `Some(true)` when the list should go back to its tail, `None`
    /// when no row was open.
    pub(super) fn close(&mut self) -> Option<bool> {
        self.said = None;
        self.open.take().map(|open| open.resume)
    }

    /// An arrow, Home or End: the cursor stands on `row`.
    pub(super) fn select(&mut self, row: &RowId, slot: usize) {
        self.cursor = Some(Cursor {
            row: row.clone(),
            slot,
        });
    }

    /// Opens `row`. A list that stood at its tail when its first row opened goes back to
    /// the tail when the last one closes.
    fn show(&mut self, row: &RowId) {
        let resume = self.open.as_ref().map_or(self.at_tail, |open| open.resume);
        self.said = None;
        self.open = Some(Open {
            row: row.clone(),
            resume,
        });
    }

    /// The viewport of a list that follows its tail says whether it shows its newest row.
    pub(super) fn saw_tail(&mut self, at_tail: bool) {
        self.at_tail = at_tail;
    }
}

/// Every list of one pane, by the part of the pane it is. A pane that is one list has the
/// part `""`. Rows of one list share one open slot; two lists never share one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lists(BTreeMap<&'static str, ListState>);

impl Lists {
    /// The state of the list `part`, once something has happened to it.
    pub fn list(&self, part: &str) -> Option<&ListState> {
        self.0.get(part)
    }

    /// The state of the list `part`, made from its default the first time it is asked for.
    pub fn list_mut(&mut self, part: &'static str) -> &mut ListState {
        self.0.entry(part).or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{ListState, RowId};

    type Check = Result<(), Box<dyn Error>>;

    fn id(text: &str) -> RowId {
        RowId::new(text)
    }

    /// Record 5.2.2, rules 1 and 2: one row open in a list, and a press on it closes it.
    #[test]
    fn a_press_opens_a_row_and_a_second_press_closes_it() -> Check {
        let mut list = ListState::default();
        assert!(!list.press(&id("a"), 0));
        assert_eq!(
            (list.open(), list.cursor()),
            (Some(&id("a")), Some(&id("a")))
        );
        list.press(&id("b"), 1);
        assert_eq!(list.open(), Some(&id("b")), "opening b left a open");
        list.press(&id("b"), 1);
        assert_eq!(list.open(), None, "a press on the open row left it open");
        assert_eq!(
            list.cursor(),
            Some(&id("b")),
            "closing a row lost the selection"
        );
        Ok(())
    }

    #[test]
    fn the_arrow_keys_move_the_cursor_without_opening_or_closing() -> Check {
        let mut list = ListState::default();
        list.press(&id("a"), 0);
        list.select(&id("b"), 1);
        assert_eq!(
            (list.open(), list.cursor()),
            (Some(&id("a")), Some(&id("b")))
        );
        assert_eq!(list.cursor_slot(), Some(1));
        assert_eq!(list.toggle_cursor(), Some(Ok(id("b"))), "Return opens");
        assert_eq!(list.open(), Some(&id("b")));
        assert_eq!(
            list.toggle_cursor(),
            Some(Err(false)),
            "Return on it closes"
        );
        assert_eq!(list.open(), None);
        assert_eq!(list.close(), None);
        assert_eq!(list.close(), None, "Esc with nothing open was taken");
        Ok(())
    }

    /// The plan's default 9: a log follows its tail unless a row is open, and again after.
    #[test]
    fn closing_a_row_goes_back_to_the_tail_only_if_the_list_stood_there() -> Check {
        let mut list = ListState::default();
        list.saw_tail(true);
        list.press(&id("a"), 0);
        list.saw_tail(false);
        list.press(&id("b"), 1);
        assert_eq!(
            list.close(),
            Some(true),
            "the tail was in view when a row first opened"
        );
        list.press(&id("a"), 0);
        assert_eq!(
            list.close(),
            Some(false),
            "the reader had left the tail before opening"
        );
        let mut plain = ListState::default();
        plain.press(&id("a"), 0);
        assert_eq!(
            plain.close(),
            Some(false),
            "a list that never follows went to its tail"
        );
        Ok(())
    }

    #[test]
    fn a_scene_can_show_a_row_open() -> Check {
        let mut list = ListState::default();
        list.set_open(Some(id("a")));
        assert_eq!(list.open(), Some(&id("a")));
        list.set_open(None);
        assert_eq!(list.open(), None);
        Ok(())
    }
}
