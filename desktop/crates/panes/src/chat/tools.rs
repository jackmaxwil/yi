//! Tool rows and the receipt an answered ask leaves under its step.

use rune::{Cursor, El, Icon, ScrollSpec, Span, TextStyle, Tx, col, rich, row, text};
use yd_core::address::uri::Address;
use yd_core::grid::step::GridMsg;
use yd_core::ids::Millis;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::pointed;
use yd_core::reduce::turns::{ToolCall, ToolState};
use yd_core::tokens::Tokens;
use yd_ui::code::doc::Lang;
use yd_ui::code::paint::{Read, paint};
use yd_ui::grid_view::view::{Act, act, open_file};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::line_icon;
use yd_ui::theme::look::{BODY, CODE, CODE_LINE, LABEL, META, colour};
use yd_ui::theme::scale::{CONSOLE, RADIUS_CHIP};
use yd_ui::win::{Win, dispatch};

use crate::chat::messages::{Frame, clean, line, plain};
use crate::chat::pane::chat_mut;

static EYE: Icon = line_icon!(
    "eye",
    r#"<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#
);
static EDIT: Icon = line_icon!("edit", r#"<path d="M4 20h4L19 9l-4-4L4 16v4z"/>"#);
static SHELL: Icon = line_icon!("shell", r#"<path d="M5 7l5 5-5 5M12 17h7"/>"#);
static TOOL: Icon = line_icon!("tool", r#"<circle cx="12" cy="12" r="3"/>"#);

/// Mono inside sans text is drawn at 0.92 of its size (record 3.0).
const TARGET: TextStyle = TextStyle::new(10.58, 400.0).lh(16.0).family(CODE);
/// Characters of a target or a result line drawn before the ellipsis.
const MAX_LINE: usize = 200;
/// Characters an opened step draws of its command, and of what it printed (the last ones).
const MAX_OPEN: usize = 20_000;
/// The tallest an opened step's command and output stand, twelve code lines; past it they
/// scroll inside the row, so an open row never takes the pane.
pub const OPEN_CAP: f32 = 12.0 * 16.0;

/// What a tool did, said as a verb, and its icon.
fn verb(name: &str) -> (&'static Icon, String) {
    match name {
        "read" => (&EYE, "Read".to_owned()),
        "show" => (&EYE, "Showed".to_owned()),
        "grep" | "find" | "glob" | "search" => (&SEARCH, "Searched".to_owned()),
        "edit" | "write" => (&EDIT, "Edited".to_owned()),
        "bash" => (&SHELL, "Ran".to_owned()),
        other => (&TOOL, line(other, 40)),
    }
}

/// What a call was aimed at, whole: a path, a quoted pattern, a command.
fn aim(call: &ToolCall) -> Option<String> {
    aimed(call).map(|(_, aim)| aim)
}

/// The aim, and the argument it was read from.
fn aimed(call: &ToolCall) -> Option<(&'static str, String)> {
    let arg = |name: &'static str| {
        let found = call.args.get(name).and_then(|v| v.as_str());
        found.map(|v| (name, v.to_owned()))
    };
    // A hashline patch names its file in its first line: `[src/main.rs#1D52]` or `[README.md]`.
    let patched = arg("patch").and_then(|(name, p)| {
        let path = p.strip_prefix('[')?.split(['#', ']']).next()?;
        Some((name, path.to_owned()))
    });
    arg("path")
        .or_else(|| arg("file_path"))
        .or(patched)
        .or_else(|| arg("pattern").map(|(name, p)| (name, format!("\"{p}\""))))
        .or_else(|| arg("command"))
        .or_else(|| arg("query"))
        .or_else(|| arg("code"))
        .or_else(|| arg("address"))
}

/// The whole call as an ask shows it: its aim (else its arguments as JSON), how that reads,
/// and every other argument, a `cwd` only when it is not `folder`.
pub(crate) fn whole(call: &ToolCall, folder: &str) -> Option<(String, Read, Option<String>)> {
    let Some((name, _)) = aimed(call) else {
        return shown_aim(call).map(|(text, read)| (text, read, None));
    };
    let args = call.args.as_object().into_iter().flatten();
    let others: Vec<String> = args
        .filter(|(key, value)| *key != name && !(*key == "cwd" && value.as_str() == Some(folder)))
        .map(|(key, value)| match value.as_str() {
            Some(words) => format!("{key} {words}"),
            None => format!("{key} {value}"),
        })
        .collect();
    let (aim, read) = shown_aim(call)?;
    Some((aim, read, (!others.is_empty()).then(|| others.join(" · "))))
}

/// How a call's aim reads: a command as shell, a cell as Python, the rest as words.
pub(crate) fn aim_read(call: &ToolCall) -> Read {
    match call.name.as_str() {
        "bash" => Read::Code(Lang::SHELL),
        "ipython" => Read::Code(Lang::PYTHON),
        _ => Read::Code(Lang::PLAIN),
    }
}

/// What an opened step shows above its output, and how it reads: the aim, else the arguments
/// of a tool that names none, as JSON.
fn shown_aim(call: &ToolCall) -> Option<(String, Read)> {
    let args = call.args.as_object().filter(|args| !args.is_empty());
    match aim(call) {
        Some(aimed) => Some((aimed, aim_read(call))),
        None => args.map(|_| (format!("{:#}", call.args), Read::Code(Lang::JSON))),
    }
}

/// What an opened step shows as its output, and how it reads: an edit's diff, a read's or a
/// search's rows, else what it printed.
fn shown_output(call: &ToolCall) -> (String, Read) {
    let patch = call.details.get("patch").and_then(|v| v.as_str());
    match (call.name.as_str(), patch) {
        ("edit" | "write", Some(patch)) => (patch.to_owned(), Read::Diff),
        ("read", _) => (output(call), Read::Numbered { hits: false }),
        ("grep", _) => (output(call), Read::Numbered { hits: true }),
        _ => (output(call), Read::Output),
    }
}

/// What a call was aimed at, as its one line says it.
pub fn target(call: &ToolCall) -> Option<String> {
    aim(call).map(|found| line(&found, MAX_LINE))
}

/// The lines of an aim past its first, which a closed row counts instead of drawing.
fn more(call: &ToolCall) -> Option<usize> {
    let lines = aim(call)?.trim_end().lines().count();
    lines.checked_sub(1).filter(|n| *n > 0)
}

/// The file a call named, as the tool was given it: under the working folder, or absolute.
fn file(call: &ToolCall) -> Option<&str> {
    let arg = |name: &str| call.args.get(name).and_then(|v| v.as_str());
    arg("path").or_else(|| arg("file_path"))
}

/// The file at `path` as the files source names it: a path inside the shown thread's folder
/// by its place in it, any other as written.
fn placed(win: &Win, path: &str) -> Option<Address> {
    let folder = win.hub.try_borrow().ok().and_then(|hub| {
        let thread = yd_ui::shown::thread(&hub, win.window)?;
        Some(yd_ui::shown::works_in(&hub, thread).to_owned())
    });
    let inside = folder.as_deref().and_then(|folder| {
        let rest = path.strip_prefix(folder)?;
        rest.strip_prefix('/')
    });
    Address::new(Kind::File, inside.unwrap_or(path)).ok()
}

/// Opens the file a step touched in the code viewer.
pub(crate) fn open(win: &mut Win, tx: &mut Tx, path: &str) {
    if let Some(file) = placed(win, path) {
        open_file(win, tx, file);
    }
}

/// The file a `show` pointed at and its line, as the model wrote the address.
fn pointed(call: &ToolCall) -> Option<(String, Option<u32>)> {
    let address = call.args.get("address").and_then(|v| v.as_str())?;
    (call.name == "show").then(|| pointed::target(address.trim(), ""))?
}

/// Brings the pane about the file the model pointed at in front, at its line, with the keys:
/// the person's own word, where the model's waited as a tab.
fn show_me(win: &mut Win, tx: &mut Tx, path: &str, line: Option<u32>) {
    let Some(thing) = placed(win, path) else {
        return;
    };
    let (window, kind) = (win.window, PaneKind::Code);
    let about = GridMsg::About {
        window,
        kind,
        thing: thing.clone(),
    };
    dispatch(win, Msg::Grid(about));
    let pane = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        let mut panes = hub.model.grid.panes.iter();
        let found = panes.find(|pane| pane.kind == kind && pane.thing.as_ref() == Some(&thing));
        let pane = found?.id;
        if let Some(line) = line {
            hub.model.reveal(pane, line);
        }
        Some(pane)
    });
    if let Some(pane) = pane {
        act(win, tx, Act::Focus(pane));
    }
}

/// The text a call printed.
pub fn output(call: &ToolCall) -> String {
    let blocks = plain!(&call.output);
    let texts = blocks.as_array().map(|blocks| {
        let words = blocks.iter().filter_map(|b| b.get("text")?.as_str());
        words.collect::<Vec<_>>().join("\n")
    });
    texts.unwrap_or_default()
}

/// The short result: a diff's counts, a read's lines, a search's matches, else its first line.
fn result(tokens: &Tokens, call: &ToolCall) -> El {
    let faint = colour(tokens.faint);
    let number = |name: &str| call.details.get(name).and_then(|v| v.as_u64());
    if let (Some(added), removed) = (number("added"), number("removed").unwrap_or(0)) {
        let plus = format!("+{added}");
        let mut words = plus.clone();
        let mut spans = vec![Span::new(0, plus.len()).color(colour(tokens.ok))];
        if removed > 0 {
            let start = words.len().saturating_add(1);
            words.push_str(&format!(" −{removed}"));
            spans.push(Span::new(start, words.len()).color(colour(tokens.bad)));
        }
        return rich(words, spans)
            .font(LABEL)
            .color(faint)
            .truncate_fade_width(0.0);
    }
    let printed = output(call);
    let words = match (call.state, call.name.as_str()) {
        (ToolState::Lost, _) => "not answered".to_owned(),
        (ToolState::Asking, _) => "waiting for you".to_owned(),
        // Yi's bash writes this when the turn's stop reached the command; a timeout says so.
        (_, "bash") if printed.lines().any(|l| l == "[command aborted]") => {
            "stopped with the turn".to_owned()
        }
        (ToolState::Queued, _) => "queued".to_owned(),
        (ToolState::Running, _) => line(printed.lines().last().unwrap_or("running"), MAX_LINE),
        (_, "read") => format!("{} lines", printed.lines().count()),
        // The answer's first clause says where it landed; the rest is for the model.
        (_, "show") => line(printed.split([';', '.']).next().unwrap_or(""), MAX_LINE),
        (_, "grep" | "find" | "glob" | "search") => {
            let found = printed.lines().filter(|l| !l.trim().is_empty()).count();
            format!("{found} matches")
        }
        _ => line(paint(&printed, Read::Output).text(), MAX_LINE),
    };
    // A long result gives way to the target, which keeps room to say what was touched.
    text(words)
        .font(LABEL)
        .color(faint)
        .truncate()
        .shrink()
        .min_w(0.0)
}

/// `m:ss` since `from`.
pub fn waited(from: Millis, now: Millis) -> String {
    let secs = now.0.saturating_sub(from.0) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// What a lost step says, and what its button asks the agent.
/// Incident: a killed app's approval is gone, and neither Stop nor a new prompt frees Yi's run.
const LOST: &str = "This approval was lost when the app quit. Yi is still waiting on it, and Stop \
    does not free it: the run stays held until yi serve restarts.";

/// A lost step's line: what happened and what frees the run.
fn lost(key: &str, tokens: &Tokens) -> El {
    let reason = text(LOST).font(META).color(colour(tokens.warn)).wrap();
    col()
        .key(format!("{key}.lost"))
        .w_full()
        .pl(19.0)
        .child(reason)
}

/// The folder a call ran in: the one it named, else the one its thread works in.
fn folder<'a>(frame: &Frame<'a>, call: &'a ToolCall) -> &'a str {
    let named = call.args.get("cwd").and_then(|v| v.as_str());
    named.unwrap_or(frame.folder)
}

/// How a call ended: a command's exit status, else where it stands.
fn ended(call: &ToolCall) -> String {
    let flag = |name: &str| call.details.get(name).and_then(|v| v.as_bool()) == Some(true);
    if flag("timedOut") {
        return "timed out".to_owned();
    }
    if let Some(code) = call.details.get("exitCode").and_then(|v| v.as_i64()) {
        return format!("exit {code}");
    }
    let state = match call.state {
        ToolState::Queued => "queued",
        ToolState::Asking => "waiting for you",
        ToolState::Running => "running",
        ToolState::Done => "done",
        ToolState::Failed => "failed",
        ToolState::Lost => "not answered",
    };
    state.to_owned()
}

/// What an opened step shows: its whole command, folder, end and time, and what it printed,
/// the last [`MAX_OPEN`] characters of it with a notice where the rest was cut.
fn opened(frame: &Frame, key: &str, call: &ToolCall) -> El {
    let tokens = frame.tokens;
    let aimed = shown_aim(call).map(|(aimed, read)| {
        let (whole, total) = clean(&aimed, MAX_OPEN);
        let cut = (total > MAX_OPEN).then(|| format!("[…] {MAX_OPEN} of {total} characters shown"));
        (paint(&whole, read), cut)
    });
    let mut facts = vec![format!("in {}", folder(frame, call)), ended(call)];
    facts.extend(call.duration().map(|ms| match ms.0 {
        0..1_000 => format!("{} ms", ms.0),
        _ => crate::chat::messages::span(ms.0),
    }));
    let (printed, read) = shown_output(call);
    let total = printed.chars().count();
    let skip = total.saturating_sub(MAX_OPEN);
    let tail: String = printed.chars().skip(skip).collect();
    let shown = paint(&tail, read);
    let notice = |words: String| text(words).font(META).color(colour(tokens.faint)).wrap();
    let cut = (skip > 0)
        .then(|| format!("[…] last {MAX_OPEN} of {total} characters shown; Copy takes them all"));
    let well = col()
        .w_full()
        .px(6.0)
        .py(4.0)
        .radius(RADIUS_CHIP.of(tokens))
        .fill(colour(CONSOLE.of(tokens)))
        .children(cut.map(notice))
        .child(
            shown
                .draw(
                    tokens,
                    &format!("{key}.output"),
                    CODE_LINE,
                    colour(tokens.dim),
                )
                .wrap(),
        )
        .children(shown.notice().map(notice));
    let aimed = aimed.map(|(aimed, cut)| {
        let words = aimed.draw(tokens, &format!("{key}.command"), TARGET, colour(tokens.fg));
        col()
            .w_full()
            .child(words.wrap())
            .children(cut.into_iter().chain(aimed.notice()).map(notice))
    });
    let body = col()
        .key(format!("{key}.body"))
        .w_full()
        .max_h(OPEN_CAP)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().w_full().gap(4.0).children(aimed).child(well));
    let copy = button(tokens, Button::Ghost, "Copy", None)
        .key(format!("{key}.copy"))
        .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(printed.clone()));
    col()
        .key(format!("{key}.detail"))
        .w_full()
        .pl(19.0)
        .pt(2.0)
        .pb(6.0)
        .gap(4.0)
        .child(
            row()
                .w_full()
                .gap(6.0)
                .center_y()
                .child(
                    text(facts.join(" · "))
                        .font(META)
                        .color(colour(tokens.faint))
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(copy),
        )
        .child(body)
}

/// One step: icon, verb, target and result, opened in place by a press or Return; an answered
/// ask leaves its receipt under it, and a lost one says so.
pub fn step(frame: &Frame, call: &ToolCall) -> El {
    let tokens = frame.tokens;
    let (icon, verb) = verb(&call.name);
    let key = format!("{}.step.{}", frame.key, call.id.as_str());
    let shown = frame.steps.contains(&call.id);
    let (pane, id, held) = (frame.pane, call.id.clone(), key.clone());
    let said = format!("{} {verb}", if shown { "Close" } else { "Open" });
    let mut step = row()
        .key(key.clone())
        .label(said)
        .w_full()
        .h(20.0)
        .gap(6.0)
        .center_y()
        .cursor(Cursor::Pointer)
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            // Invariant: what is above an opened step stays where it was drawn.
            tx.keep_on_screen(held.clone());
            chat_mut(win, pane, |chat| {
                match chat.steps.iter().position(|step| *step == id) {
                    Some(at) => {
                        chat.steps.remove(at);
                    }
                    None => chat.steps.push(id.clone()),
                }
            });
        })
        .child(glyph(icon, 13.0, tokens.dim))
        .child(
            text(verb)
                .key(format!("{key}.verb"))
                .font(BODY)
                .color(colour(tokens.fg))
                .min_w(50.0),
        )
        .children(target(call).map(|t| {
            let words = paint(&t, aim_read(call))
                .draw(tokens, &format!("{key}.target"), TARGET, colour(tokens.dim))
                .truncate()
                .shrink()
                .min_w(0.0);
            // A step that names a file opens it; a pattern or a command is only words.
            let words = match (file(call).map(str::to_owned), pointed(call)) {
                (Some(path), _) => words
                    .key(format!("{key}.path"))
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |win: &mut Win, tx: &mut Tx| open(win, tx, &path)),
                (None, Some((path, at))) => words
                    .key(format!("{key}.path"))
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |win: &mut Win, tx: &mut Tx| show_me(win, tx, &path, at)),
                (None, None) => words,
            };
            let more = more(call).map(|n| {
                let said = format!("+{n} {}", if n == 1 { "line" } else { "lines" });
                let more = text(said).key(format!("{key}.more")).font(META);
                more.color(colour(tokens.faint))
            });
            let aimed = row().grow(1.0).min_w(60.0).gap(6.0).center_y();
            aimed.child(words).children(more)
        }))
        .children(
            pointed(call)
                .filter(|_| call.state == ToolState::Done)
                .map(|(path, at)| {
                    text("Show me")
                        .key(format!("{key}.show"))
                        .label("Show me")
                        .font(LABEL)
                        .color(colour(tokens.accent))
                        .cursor(Cursor::Pointer)
                        .on_press_tx(move |win: &mut Win, tx: &mut Tx| show_me(win, tx, &path, at))
                }),
        )
        .child(result(tokens, call));
    if let Some(ms) = call.duration().map(|d| d.0).filter(|ms| *ms >= 1_000) {
        let words = crate::chat::messages::span(ms);
        step = step.child(text(words).font(META).color(colour(tokens.faint)));
    }
    let receipt = call.gate.and_then(|gate| {
        let (allowed, answered) = (gate.allowed?, gate.answered?);
        let said = if allowed { "Allowed" } else { "Denied" };
        let words = format!("{said} · waited {}", waited(gate.asked, answered));
        let ink = if allowed { tokens.ok } else { tokens.bad };
        Some(
            row()
                .key(format!("{key}.receipt"))
                .h(16.0)
                .pl(19.0)
                .center_y()
                .child(text(words).font(META).color(colour(ink)).truncate()),
        )
    });
    let lost = (call.state == ToolState::Lost).then(|| lost(&key, tokens));
    let opened = shown.then(|| opened(frame, &key, call));
    col()
        .w_full()
        .child(step)
        .children(receipt)
        .children(lost)
        .children(opened)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::reduce::turns::ToolCall;

    use super::{target, verb, waited};
    use yd_core::ids::Millis;

    #[test]
    fn a_step_names_what_it_touched() -> Result<(), Box<dyn Error>> {
        let call = ToolCall {
            args: "{\"pattern\":\"RateLimit\"}".parse()?,
            ..ToolCall::default()
        };
        assert_eq!(target(&call).as_deref(), Some("\"RateLimit\""));
        assert_eq!(verb("grep").1, "Searched");
        assert_eq!(verb("edit").1, "Edited");
        assert_eq!(waited(Millis(1_000), Millis(43_000)), "0:42");
        Ok(())
    }
}
