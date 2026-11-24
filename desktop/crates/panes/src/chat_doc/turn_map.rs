//! The turn map at the right edge: every turn and fork in order, the current one marked, and
//! the place the thread needs you (record 3.1.10).

use rune::{Cursor, El, Tx, col, el, row, text};
use yd_core::ids::ThreadId;
use yd_core::reduce::state::Wire;
use yd_core::reduce::threads::{Status, Thread};
use yd_core::reduce::turns::Turn;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::theme::look::{LABEL, colour};
use yd_ui::theme::scale::LINE_QUIET;
use yd_ui::win::Win;

/// One entry of the map, in conversation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Turn { number: u32, current: bool },
    Fork(ThreadId),
    NeedsYou,
}

/// The entries for `thread`: its turns and the forks made from it, by time, then whether it
/// waits on you.
pub fn entries(wire: &Wire, thread: &Thread, turns: &[Turn]) -> Vec<Entry> {
    let last = turns.last().map(|turn| turn.number);
    let mut timed: Vec<(u64, Entry)> = turns
        .iter()
        .map(|turn| {
            let current = Some(turn.number) == last;
            let entry = Entry::Turn {
                number: turn.number,
                current,
            };
            (turn.started.0, entry)
        })
        .collect();
    let forks = wire.threads.values();
    let forks = forks.filter(|t| t.forked_from.as_ref() == Some(&thread.id));
    timed.extend(forks.map(|fork| (fork.last_activity.0, Entry::Fork(fork.id.clone()))));
    timed.sort_by_key(|(at, _)| *at);
    let mut out: Vec<Entry> = timed.into_iter().map(|(_, entry)| entry).collect();
    if wire.status(&thread.id) == Status::NeedsYou {
        out.push(Entry::NeedsYou);
    }
    out
}

/// Where a press on an entry goes: a turn's block, a fork's own thread, the bottom stack.
fn press(win: &mut Win, tx: &mut Tx, entry: &Entry, key: &str) {
    match entry {
        Entry::Turn { number, current } => {
            let part = if *current {
                "title".to_owned()
            } else {
                format!("block.turn.{number}")
            };
            tx.scroll_into_view(format!("{key}.{part}"));
        }
        Entry::Fork(id) => yd_ui::shown::show(win, id.clone()),
        Entry::NeedsYou => tx.scroll_to_bottom(format!("{key}.doc")),
    }
}

/// The map: quiet labels, each followed by a short bar.
pub fn map(key: &str, tokens: &Tokens, entries: &[Entry]) -> El {
    let quiet = LINE_QUIET.of(tokens);
    let item = |entry: &Entry, part: String| {
        let (words, ink, bar, long): (String, Rgba, Rgba, bool) = match entry {
            Entry::Turn {
                number,
                current: true,
            } => (format!("Turn {number}"), tokens.fg, tokens.fg, true),
            Entry::Turn { number, .. } => (format!("Turn {number}"), tokens.faint, quiet, false),
            Entry::Fork(_) => ("Fork".to_owned(), tokens.faint, quiet, false),
            Entry::NeedsYou => ("needs you".to_owned(), tokens.need, tokens.need, false),
        };
        let entry = entry.clone();
        let root = key.to_owned();
        row()
            .key(format!("{key}.map.{part}"))
            .gap(7.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| press(win, tx, &entry, &root))
            .child(text(words).font(LABEL).color(colour(ink)))
            .child(
                el().size(if long { 22.0 } else { 14.0 }, 2.0)
                    .radius(1.0)
                    .fill(colour(bar)),
            )
    };
    col()
        .key(format!("{key}.map"))
        .gap(9.0)
        .children(entries.iter().map(|entry| {
            let part = match entry {
                Entry::Turn { number, .. } => format!("turn-{number}"),
                // Invariant: `/` separates the elements of a path in Rune's tree, so a key holds none.
                Entry::Fork(id) => format!("fork-{}", id.as_str().replace('/', ":")),
                Entry::NeedsYou => "needs-you".to_owned(),
            };
            item(entry, part)
        }))
}
