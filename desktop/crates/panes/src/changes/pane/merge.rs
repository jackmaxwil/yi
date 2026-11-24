//! Bringing a Yi checkout's work into the project: the bar's button, the question it asks
//! first, and what the merge said. The `git` source checks again and merges.

use rune::{Cursor, El, col, row, text};
use yd_core::address::attach::AttachMsg;
use yd_core::command::table::{Route, Run, Target};
use yd_core::command::tools::changes::MERGE;
use yd_core::feed::{SourceCmd, Table};
use yd_core::ids::{PaneId, RowId};
use yd_core::msg::Msg;
use yd_core::reduce::step::commands::PROMPT;
use yd_core::row::Row;
use yd_core::tokens::Tokens;
use yd_ui::kit::button::{Button, button};
use yd_ui::theme::look::{META, colour};
use yd_ui::win::{Win, dispatch};

use super::{FEED, change};

/// What "Ask it to commit" sends the agent.
const ASK_COMMIT: &str = "Please commit your changes with a short message, so I can merge them.";

/// Where the pane's merge stands: its question asked, or sent after `done` earlier merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    Asked,
    Sent(u64),
}

/// The source's `merge` row: there only while the thread works in a Yi checkout.
fn of(table: Option<&Table>) -> Option<&Row> {
    table?.find(&RowId::new("merge"))
}

fn done(row: &Row) -> u64 {
    row.words("done").and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// `n` and the noun, which takes an `s` past one.
fn some(n: &str, noun: &str) -> String {
    let s = if n == "1" { "" } else { "s" };
    format!("{n} {noun}{s}")
}

/// "Merge into main", for a thread that works in a Yi checkout.
pub fn button_of(tokens: &Tokens, key: &str, pane: PaneId, table: Option<&Table>) -> Option<El> {
    let base = of(table)?.words("base")?;
    let words = format!("Merge into {base}");
    let ask = move |win: &mut Win| set(win, pane, Merge::Asked);
    Some(
        button(tokens, Button::Plain, &words, None)
            .key(format!("{key}.merge"))
            .label(words)
            .on_press(ask),
    )
}

/// Where the merge stands, for the thread the pane shows.
fn set(win: &mut Win, pane: PaneId, merge: Merge) {
    let shown = win.hub.try_borrow().ok().and_then(|hub| {
        let thread = yd_ui::shown::of_pane(&hub, pane)?;
        Some(thread.id.clone())
    });
    change(win, pane, |s| s.merge = Some((shown, merge)));
}

/// The merge goes to the source, which checks again before git runs.
fn yes(win: &mut Win, pane: PaneId) {
    let done = win
        .hub
        .try_borrow()
        .ok()
        .map_or(0, |hub| of(hub.model.tables.get(FEED)).map_or(0, done));
    set(win, pane, Merge::Sent(done));
    dispatch(win, Msg::Source(FEED, SourceCmd::Run(Run::new(MERGE))));
}

/// Asks the shown thread to commit its work, which is what a merge takes. What is attached
/// waits for the person's own next prompt.
fn commit(win: &mut Win, pane: PaneId) {
    let (run, kept) = {
        let Ok(hub) = win.hub.try_borrow() else {
            return;
        };
        let Some(thread) = yd_ui::shown::of_pane(&hub, pane) else {
            return;
        };
        let Ok(address) = thread.address() else {
            return;
        };
        let run = Run {
            text: Some(ASK_COMMIT.to_owned()),
            ..Run::new(PROMPT).on(Target::Thing(address))
        };
        (run, hub.model.attached.things.clone())
    };
    dispatch(win, Msg::Attach(AttachMsg::Clear));
    dispatch(win, Msg::Command(Route::Wire, run));
    for thing in kept {
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
    }
    change(win, pane, |s| s.merge = None);
}

/// Under the bar: the question, why no merge can be made, or what the merge said.
pub fn line(
    tokens: &Tokens,
    key: &str,
    pane: PaneId,
    table: Option<&Table>,
    merge: Option<Merge>,
) -> Option<El> {
    let merged = of(table)?;
    let cell = |name| merged.words(name).unwrap_or_default();
    let refused = merged.words("refused").filter(|why| !why.is_empty());
    let (words, asks) = match merge? {
        Merge::Asked => match refused {
            Some(why) => (why.to_owned(), false),
            None => {
                let (commits, files) =
                    (some(cell("commits"), "commit"), some(cell("files"), "file"));
                let (branch, base, home) = (cell("branch"), cell("base"), cell("home"));
                let words = format!(
                    "Merge {branch} into {base} in {home}? {commits}, {files}. A local merge: nothing is pushed."
                );
                (words, true)
            }
        },
        Merge::Sent(before) if done(merged) > before => (cell("said").to_owned(), false),
        Merge::Sent(_) => ("Merging…".to_owned(), false),
    };
    let ask = format!("{key}.merge-ask");
    let press = |part: &str, words: &str, ink, go: fn(&mut Win, PaneId)| {
        text(words.to_owned())
            .key(format!("{ask}.{part}"))
            .font(META)
            .color(colour(ink))
            .cursor(Cursor::Pointer)
            .on_press(move |win: &mut Win| go(win, pane))
    };
    let close: fn(&mut Win, PaneId) = |win, pane| change(win, pane, |s| s.merge = None);
    let sending = matches!(merge, Some(Merge::Sent(before)) if done(merged) <= before);
    let uncommitted = refused.is_some() && merged.words("uncommitted") == Some("1");
    let commit = uncommitted.then(|| press("commit", "Ask it to commit", tokens.accent, commit));
    let buttons = match (asks, sending) {
        (true, _) => vec![
            press("yes", "Merge", tokens.accent, yes),
            press("no", "Cancel", tokens.dim, close),
        ],
        (false, false) => commit
            .into_iter()
            .chain([press("no", "OK", tokens.dim, close)])
            .collect(),
        (false, true) => Vec::new(),
    };
    // Incident: a project path has no break in it and ran under the buttons beside it.
    let words = text(words).font(META).color(colour(tokens.dim));
    let buttons = row().gap(8.0).self_end().children(buttons);
    Some(
        col()
            .key(ask.clone())
            .w_full()
            .gap(2.0)
            .px(8.0)
            .py(4.0)
            .child(words.wrap_anywhere().min_w(0.0))
            .child(buttons),
    )
}
