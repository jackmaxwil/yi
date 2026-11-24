//! The steps line's pop-over, and the diff card the conversation and the pop-over share.

use rune::{Align, Cursor, El, Icon, Side, Span, TextStyle, Tx, col, el, rich, row, spacer, text};
use rune_kit::diff::{FileStatus, LineKind, parse};
use yd_core::address::uri::Address;
use yd_core::kind::Kind;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::{ToolCall, Turn};
use yd_core::tokens::Tokens;
use yd_ui::grid_view::view::open_file;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::line_icon;
use yd_ui::theme::look::{CODE, READ_SMALL, colour};
use yd_ui::theme::scale::{LINE_CARD, LINE_MENU, LINE_SOFT, POPOVER, SEMIBOLD, SHADOW_MENU, STEP};
use yd_ui::win::Win;

use crate::chat::messages::{Block as Said, clock, line, said};
use crate::chat::tools::{output, target};

static EYE: Icon = line_icon!(
    "eye",
    r#"<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#
);
static EDIT: Icon = line_icon!("edit", r#"<path d="M4 20h4L19 9l-4-4L4 16v4z"/>"#);
static SHELL: Icon = line_icon!("shell", r#"<path d="M5 7l5 5-5 5M12 17h7"/>"#);
static TOOL: Icon = line_icon!("tool", r#"<circle cx="12" cy="12" r="3"/>"#);
pub static FILE: Icon = line_icon!(
    "file",
    r#"<path d="M14 3H6v18h12V7z"/><path d="M14 3v4h4"/>"#
);

/// Diff lines: 11.5 mono on an 18 line (record 3.1.9).
const CODE_ROW: TextStyle = TextStyle::new(11.5, 400.0).lh(18.0).family(CODE);
/// Mono inside 12 px sans is drawn at 0.92 of it.
const TARGET: TextStyle = TextStyle::new(11.04, 400.0).lh(20.0).family(CODE);
const TIME: TextStyle = TextStyle::new(10.5, 400.0).lh(20.0).family(CODE);
/// Lines a card in the pop-over shows, and a card in the conversation.
pub const POPOVER_LINES: usize = 3;
pub const COLUMN_LINES: usize = 2;
const MAX_LINE: usize = 200;

/// The tool calls a turn made, in the order it made them.
pub fn calls(thread: &Thread, turn: Turn) -> Vec<&ToolCall> {
    let all = &thread.transcript.messages;
    let range = all
        .get(turn.first..)
        .unwrap_or_default()
        .iter()
        .take(turn.len);
    range
        .filter_map(said)
        .flat_map(|said| said.blocks)
        .filter_map(|block| match block {
            Said::Call(id) => thread.transcript.tools.get(id.as_str()),
            Said::Text(_) | Said::Thinking(_) => None,
        })
        .collect()
}

fn number(call: &ToolCall, name: &str) -> Option<u64> {
    call.details.get(name).and_then(|v| v.as_u64())
}

/// The calls that changed a file, the files, and the lines they added and took out.
pub fn changed<'a>(calls: &[&'a ToolCall]) -> (Vec<&'a ToolCall>, usize, u64, u64) {
    let edits: Vec<&ToolCall> = calls
        .iter()
        .copied()
        .filter(|call| number(call, "added").is_some())
        .collect();
    let mut paths: Vec<String> = edits.iter().filter_map(|call| target(call)).collect();
    paths.sort();
    paths.dedup();
    let sum = |name: &str| {
        edits
            .iter()
            .filter_map(|call| number(call, name))
            .fold(0u64, u64::saturating_add)
    };
    (edits.clone(), paths.len(), sum("added"), sum("removed"))
}

/// `+A −R` in the plus and minus colours; `−R` only when something was taken out.
pub fn counts(tokens: &Tokens, added: u64, removed: u64, style: TextStyle) -> El {
    let plus = format!("+{added}");
    let mut words = plus.clone();
    let mut spans = vec![Span::new(0, plus.len()).color(colour(tokens.ok))];
    if removed > 0 {
        let start = words.len().saturating_add(1);
        words.push_str(&format!(" −{removed}"));
        spans.push(Span::new(start, words.len()).color(colour(tokens.bad)));
    }
    rich(words, spans).font(style).truncate_fade_width(0.0)
}

fn verb(name: &str) -> (&'static Icon, String) {
    match name {
        "read" => (&EYE, "Read".to_owned()),
        "grep" | "find" | "glob" | "search" => (&SEARCH, "Searched".to_owned()),
        "edit" | "write" => (&EDIT, "Edited".to_owned()),
        "bash" => (&SHELL, "Ran".to_owned()),
        other => (&TOOL, line(other, 40)),
    }
}

/// The short result as the record writes it: a read's file length, a search's matches and
/// files, an edit's counts, else the first line printed.
fn result(tokens: &Tokens, call: &ToolCall) -> El {
    let dim = colour(tokens.dim);
    if let Some(added) = number(call, "added") {
        let removed = number(call, "removed").unwrap_or(0);
        return counts(tokens, added, removed, READ_SMALL.lh(20.0));
    }
    let printed = output(call);
    let words = match call.name.as_str() {
        "read" => number(call, "lines").map_or_else(
            || format!("{} lines", printed.lines().count()),
            |n| format!("{n} lines"),
        ),
        "grep" | "find" | "glob" | "search" => {
            let found = printed.lines().filter(|l| !l.trim().is_empty()).count();
            match number(call, "files") {
                Some(files) => format!("{found} matches in {files} files"),
                None => format!("{found} matches"),
            }
        }
        _ => line(&printed, MAX_LINE),
    };
    text(words).font(READ_SMALL.lh(20.0)).color(dim).truncate()
}

/// One code line of a card: number, sign and text on the change's colour.
fn code_line(tokens: &Tokens, number: Option<u32>, kind: LineKind, words: &str) -> El {
    let (sign, fill) = match kind {
        LineKind::Added => ("+", Some(tokens.added)),
        LineKind::Removed => ("-", Some(tokens.removed)),
        LineKind::Context => (" ", None),
    };
    let faint = colour(tokens.faint);
    let mut out = row()
        .w_full()
        .h(18.0)
        .center_y()
        .child(
            row().w(28.0).pr(8.0).justify_end().child(
                text(number.map_or_else(String::new, |n| n.to_string()))
                    .font(CODE_ROW)
                    .color(faint),
            ),
        )
        .child(text(sign).font(CODE_ROW).color(faint).w(11.0))
        .child(
            text(line(words, MAX_LINE))
                .font(CODE_ROW)
                .color(colour(tokens.fg))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
    if let Some(fill) = fill {
        out = out.fill(colour(fill));
    }
    out
}

/// The lines a card shows: an edit's changed lines, else the lines a read printed.
pub fn lines(tokens: &Tokens, call: &ToolCall, cap: usize) -> Vec<El> {
    if let Some(patch) = call.details.get("patch").and_then(|v| v.as_str()) {
        let files = parse(patch);
        let changes = files
            .iter()
            .flat_map(|file| &file.hunks)
            .flat_map(|hunk| &hunk.lines)
            .filter(|l| l.kind != LineKind::Context);
        return changes
            .take(cap)
            .map(|l| code_line(tokens, l.new.or(l.old), l.kind, &l.text))
            .collect();
    }
    if call.name != "read" {
        return Vec::new();
    }
    let from = call
        .args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    let printed = output(call);
    let numbered = printed.lines().zip(from..).take(cap);
    numbered
        .map(|(words, n)| code_line(tokens, u32::try_from(n).ok(), LineKind::Context, words))
        .collect()
}

/// A card's frame: dark, ringed, rounded, clipping its lines.
fn frame(tokens: &Tokens, header: Option<El>, body: Vec<El>) -> El {
    col()
        .w_full()
        .radius(10.0)
        .clip_rounded(10.0)
        .fill(colour(STEP.of(tokens)))
        .border(1.0, colour(LINE_SOFT.of(tokens)))
        .children(header)
        .children(body)
}

/// The diff card in the conversation: the file, its state, its counts and Open.
pub fn card(key: &str, tokens: &Tokens, call: &ToolCall) -> Option<El> {
    let path = target(call)?;
    let patch = call.details.get("patch").and_then(|v| v.as_str());
    let created = patch.is_some_and(|p| {
        parse(p)
            .first()
            .is_some_and(|f| f.status == FileStatus::Added)
    });
    let added = number(call, "added").unwrap_or(0);
    let removed = number(call, "removed").unwrap_or(0);
    let file = path.clone();
    let open = chip(tokens, Chip::Plain, "Open", None, false)
        .key(format!("{key}.open"))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            if let Ok(address) = Address::new(Kind::File, file.as_str()) {
                open_file(win, tx, address);
            }
        });
    let header = row()
        .key(format!("{key}.header"))
        .w_full()
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(LINE_CARD.of(tokens)))
        .child(glyph(&FILE, 11.0, tokens.dim))
        .child(
            text(path)
                .font(TextStyle::new(11.04, SEMIBOLD).family(CODE))
                .color(colour(tokens.fg))
                .truncate()
                .min_w(0.0),
        )
        .children(created.then(|| text("new file").font(READ_SMALL).color(colour(tokens.dim))))
        .child(spacer())
        .child(counts(tokens, added, removed, CODE_ROW))
        .child(open);
    Some(frame(
        tokens,
        Some(header),
        lines(tokens, call, COLUMN_LINES),
    ))
}

/// One row of the pop-over: when, what, on what, how it came out, and its lines.
fn step(key: &str, tokens: &Tokens, call: &ToolCall) -> El {
    let (icon, verb) = verb(&call.name);
    let at = call.started.map(clock).unwrap_or_default();
    let body = lines(tokens, call, POPOVER_LINES);
    let card = (!body.is_empty()).then(|| frame(tokens, None, body));
    row()
        .key(format!("{key}.step.{}", call.id.as_str()))
        .w_full()
        .gap(10.0)
        .child(
            row()
                .w(58.0)
                .justify_end()
                .child(text(at).font(TIME).color(colour(tokens.faint))),
        )
        .child(row().w(16.0).pt(4.0).child(glyph(icon, 12.0, tokens.faint)))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(5.0)
                .child(
                    row()
                        .h(20.0)
                        .gap(8.0)
                        .center_y()
                        .child(
                            text(verb)
                                .font(TextStyle::new(12.0, SEMIBOLD).lh(20.0))
                                .color(colour(tokens.fg)),
                        )
                        .children(target(call).map(|t| {
                            text(t)
                                .font(TARGET)
                                .color(colour(tokens.dim))
                                .truncate()
                                .min_w(0.0)
                        }))
                        .child(result(tokens, call)),
                )
                .children(card),
        )
}

/// Every step of the turn with its time and result, drawn over what follows it, under
/// the element keyed `anchor`.
pub fn popover(
    key: &str,
    tokens: &Tokens,
    calls: &[&ToolCall],
    anchor: &str,
    keep: impl Fn(&mut Win, bool) + 'static,
) -> El {
    col()
        .key(format!("{key}.popover"))
        .anchored(anchor, Side::Bottom, Align::Start, 4.0)
        .w(746.0)
        .pt(10.0)
        .pr(14.0)
        .pb(12.0)
        .pl(2.0)
        .gap(6.0)
        .radius(10.0)
        .fill(colour(POPOVER.of(tokens)))
        .border(1.0, colour(LINE_MENU.of(tokens)))
        .box_shadow(rune::Shadow {
            y: 18.0,
            blur: 40.0,
            spread: -12.0,
            color: colour(SHADOW_MENU.of(tokens)),
            ..rune::Shadow::default()
        })
        .cursor(Cursor::Default)
        .on_hover_change(move |win: &mut Win, inside, _tx: &mut Tx| keep(win, inside))
        .children(calls.iter().map(|call| step(key, tokens, call)))
}

/// Where the pop-over hangs: 10 points wider than the column on each side, under the line.
pub fn anchor(key: &str) -> El {
    el().key(key.to_owned())
        .pin(Some(-10.0), Some(17.0), Some(-10.0), None)
        .h(0.0)
}

/// The line itself: a chevron and "Worked for … · N steps", bright while it is pointed at.
pub fn summary(tokens: &Tokens, words: String, lit: bool) -> El {
    let ink = if lit { tokens.fg } else { tokens.faint };
    row()
        .h(17.0)
        .gap(6.0)
        .center_y()
        .child(glyph(&crate::chat_doc::view::CHEVRON, 11.0, ink))
        .child(text(words).font(READ_SMALL).color(colour(ink)))
}
