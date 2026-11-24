//! Arrows move rows, Return opens one and closes it again, Esc closes it; a press does both.

use rune::{Key, KeyEvent, Tx};
use yd_core::ids::RowId;

use super::row;
use super::state::{ListState, Lists};
use super::view::Frame;
use crate::overlay::peek;
use crate::win::Win;

/// After a row opened or the cursor moved: at once, and scrolled to if it is out of view.
fn reveal(frame: &Frame, row: &RowId, tx: &mut Tx) {
    tx.instant();
    tx.scroll_into_view(row::key(&frame.key, row));
}

/// After the open row closed: at once, and back at the tail if the list was following it.
fn closed(frame: &Frame, to_tail: bool, tx: &mut Tx) {
    tx.instant();
    if to_tail && frame.follow {
        tx.scroll_to_bottom_instant(frame.rows_key());
    }
}

/// A press on a row: it is selected, and opens unless it was the open row, which closes.
pub(super) fn press(frame: &Frame, win: &mut Win, row: &RowId, slot: usize, tx: &mut Tx) {
    let pressed = |state: &mut ListState| (state.press(row, slot), state.open().is_some());
    match frame.change(win, pressed) {
        Some((_, true)) => reveal(frame, row, tx),
        Some((to_tail, false)) => closed(frame, to_tail, tx),
        None => {}
    }
}

/// Where the cursor goes for an arrow, Home or End: the row and its slot.
fn target(frame: &Frame, win: &Win, key: &Key) -> Option<(RowId, usize)> {
    let hub = win.hub.try_borrow().ok()?;
    let lists = hub.panes.get::<Lists>(Some(frame.pane));
    let state = lists.and_then(|lists| lists.list(frame.part));
    let cursor = state.and_then(|state| Some((state.cursor()?, state.cursor_slot())));
    let (len, at) = frame.place(&hub, cursor)?;
    let last = len.checked_sub(1)?;
    let slot = match (key, at) {
        (Key::Home, _) | (Key::Down, None) => 0,
        (Key::End, _) | (Key::Up, None) => last,
        (Key::Down, Some(at)) => at.saturating_add(1).min(last),
        (Key::Up, Some(at)) => at.saturating_sub(1),
        _ => return None,
    };
    let (row, _) = frame.at(&hub, slot)?;
    Some((row.id.clone(), slot))
}

/// A key while the list has the keyboard. True when the list took it.
pub(super) fn key(frame: &Frame, win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    if event.mods.cmd || event.mods.ctrl || event.mods.alt {
        return false;
    }
    if event.key == Key::Escape && peek::close(win) {
        return true;
    }
    match &event.key {
        Key::Text(typed) if typed == " " => {
            let hub = win.hub.try_borrow().ok();
            let lists = hub
                .as_ref()
                .and_then(|hub| hub.panes.get::<Lists>(Some(frame.pane)));
            let state = lists.and_then(|lists| lists.list(frame.part));
            let cursor = state
                .and_then(ListState::cursor)
                .map(|at| row::key(&frame.key, at));
            drop(hub);
            peek::toggle(win, tx, cursor);
            true
        }
        Key::Up | Key::Down | Key::Home | Key::End => {
            let Some((row, slot)) = target(frame, win, &event.key) else {
                return false;
            };
            frame.change(win, |state| state.select(&row, slot));
            reveal(frame, &row, tx);
            true
        }
        Key::Enter => match frame.change(win, ListState::toggle_cursor).flatten() {
            Some(Ok(row)) => {
                reveal(frame, &row, tx);
                true
            }
            Some(Err(to_tail)) => {
                closed(frame, to_tail, tx);
                true
            }
            None => false,
        },
        Key::Escape => {
            let Some(to_tail) = frame.change(win, ListState::close).flatten() else {
                return false;
            };
            closed(frame, to_tail, tx);
            true
        }
        _ => false,
    }
}
