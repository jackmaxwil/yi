//! The approval card: three rows, a queue of one of N, and its keys (record 3.5).

use rune::{
    Cursor, El, Key, KeyEvent, ScrollSpec, Span, TextCase, TextStyle, col, rich, row, text,
};
use yd_core::command::table::{Route, Run, Target};
use yd_core::ids::{ApprovalId, CommandId, Millis, PaneId};
use yd_core::msg::Msg;
use yd_core::reduce::approvals::{Ask, Change, Offer, OptionKind};
use yd_core::reduce::step::commands::{ALLOW, ALWAYS, ANSWER, DENY};
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::ToolCall;
use yd_core::row::Dot;
use yd_core::tokens::Tokens;
use yd_ui::code::doc::Lang;
use yd_ui::code::paint::{Read, paint};
use yd_ui::kit::button::{Button, wrapping};
use yd_ui::kit::status::thread_dot;
use yd_ui::theme::look::{META, colour};
use yd_ui::theme::scale::{RADIUS_CARD, RADIUS_CHIP, SEMIBOLD, SOFT, WELL};
use yd_ui::win::{Win, dispatch};

use crate::chat::messages::{clean, line};
use crate::chat::pane::{Chat, shown};
use crate::chat::tools::{waited, whole};

const MAX_LINE: usize = 300;
/// Characters of a command the card draws, wrapped; the rest is named where it is cut.
const MAX_COMMAND: usize = 2_000;
/// Lines of a call the card's well stands, as an open step's do; past them it scrolls.
const COMMAND_LINES: usize = 12;
/// The command's well never squeezes below this: buttons that leave it less wrap under it.
const COMMAND_MIN: f32 = 150.0;

/// The card's sizes, which the record measures twice (3.5.3).
struct Sizes {
    /// Left, right, top and bottom.
    pad: (f32, f32, f32),
    gap: f32,
    radius: Option<f32>,
    /// Rows 1 and 3, and row 1's height.
    meta: TextStyle,
    head: f32,
    kind: TextStyle,
    command: TextStyle,
    /// The well's height and its padding each side.
    well: (f32, f32),
}

const fn mono(size: f32, line: f32) -> TextStyle {
    TextStyle::new(size, 400.0)
        .lh(line)
        .family(yd_ui::theme::look::CODE)
}

/// In a pane: 9.5 text, the command 11.5 mono on a 20 line.
const PANE: Sizes = Sizes {
    pad: (11.0, 7.0, 5.0),
    gap: 4.0,
    radius: None,
    meta: META.lh(13.0),
    head: 14.0,
    kind: TextStyle::new(9.5, SEMIBOLD).lh(14.0).tracking(0.57),
    command: mono(11.5, 20.0),
    well: (22.0, 8.0),
};
/// In the document: 10.5 text on a 15 line, the command 13 mono on a 26 line.
const WIDE: Sizes = Sizes {
    pad: (15.0, 10.0, 8.0),
    gap: 6.0,
    radius: Some(9.0),
    meta: TextStyle::new(10.5, 400.0).lh(15.0),
    head: 15.0,
    kind: TextStyle::new(10.5, SEMIBOLD).lh(15.0).tracking(0.63),
    command: mono(13.0, 26.0),
    well: (28.0, 10.0),
};

/// The asks of `thread` still waiting, oldest first, and the one the card shows.
pub fn queue<'a>(asks: &'a [Ask], thread: &Thread, pick: usize) -> (Vec<&'a Ask>, usize) {
    let waiting: Vec<&Ask> = asks
        .iter()
        .filter(|ask| ask.answer.is_none() && ask.thread == thread.id)
        .collect();
    let pick = pick.min(waiting.len().saturating_sub(1));
    (waiting, pick)
}

/// The tool call an ask gates, in the thread's transcript or a child's.
fn gated<'a>(thread: &'a Thread, ask: &Ask) -> Option<&'a ToolCall> {
    let id = ask.tool.as_ref()?;
    let children = thread.agents.iter().map(|agent| &agent.transcript);
    let mut all = std::iter::once(&thread.transcript).chain(children);
    all.find_map(|transcript| transcript.tools.get(id.as_str()))
}

/// The card's label for an ask: what kind of thing it wants.
fn kind(call: Option<&ToolCall>, ask: &Ask) -> String {
    match call.map(|call| call.name.as_str()) {
        Some("bash") => "Run a command".to_owned(),
        Some("edit" | "write") => "Edit a file".to_owned(),
        _ => line(&ask.title, 80),
    }
}

/// How the card draws an offered answer: its button, its word on a command's card, its
/// key hint, and the last part of its key.
fn drawn(offer: &Offer<'_>) -> (Button, Option<&'static str>, Option<&'static str>, String) {
    let named = |command: CommandId| command.0.to_owned();
    match &offer.kind {
        OptionKind::AllowOnce => (Button::Primary, Some("Run"), Some("↵"), named(ALLOW)),
        OptionKind::AllowAlways => (Button::Plain, Some("Always"), None, named(ALWAYS)),
        OptionKind::RejectOnce => (Button::Ghost, Some("Deny"), None, named(DENY)),
        // No command names these two: the key is the option's own id.
        OptionKind::RejectAlways => (Button::Ghost, Some("Never"), None, own_key(offer)),
        // A kind a newer server offers keeps the server's own word for it.
        OptionKind::Other(_) => (Button::Ghost, None, None, own_key(offer)),
    }
}

/// Invariant: `/` separates the elements of a path in Rune's tree, so a key holds none.
fn own_key(offer: &Offer<'_>) -> String {
    format!("option.{}", offer.id.0.replace('/', ":"))
}

fn run(win: &mut Win, ask: &ApprovalId, command: CommandId, text: Option<String>) {
    let Ok(address) =
        yd_core::address::uri::Address::new(yd_core::kind::Kind::Approval, ask.as_str())
    else {
        return;
    };
    let run = Run {
        command,
        target: Target::Thing(address),
        text,
    };
    dispatch(win, Msg::Command(Route::Wire, run));
}

/// Answers by what a key means: allow once, always, or deny.
fn answer(win: &mut Win, ask: &ApprovalId, command: CommandId) {
    run(win, ask, command, None);
}

/// Answers with one of the options the server offered, by its id.
fn answer_with(win: &mut Win, ask: &ApprovalId, option: &str) {
    run(win, ask, ANSWER, Some(option.to_owned()));
}

/// The ask pane `pane` shows, if any.
pub(crate) fn showing(win: &Win, pane: PaneId) -> Option<ApprovalId> {
    let hub = win.hub.try_borrow().ok()?;
    let thread = shown(&hub, pane)?;
    let pick = hub
        .panes
        .get::<Chat>(Some(pane))
        .map_or(0, |chat| chat.pick);
    let (waiting, pick) = queue(&hub.model.wire.asks, thread, pick);
    waiting.get(pick).map(|ask| ask.id.clone())
}

/// Return runs the shown ask, cmd+Return always allows it, Esc denies it (plan default 10).
pub fn key(win: &mut Win, pane: PaneId, event: &KeyEvent) -> bool {
    let command = match (&event.key, event.mods.cmd) {
        (Key::Enter, false) => ALLOW,
        (Key::Enter, true) => ALWAYS,
        (Key::Escape, _) => DENY,
        _ => return false,
    };
    let Some(ask) = showing(win, pane) else {
        return false;
    };
    answer(win, &ask, command);
    true
}

fn step(win: &mut Win, pane: PaneId, by: isize) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    if let Some(chat) = hub.panes.get_mut::<Chat>(Some(pane)) {
        chat.pick = chat.pick.saturating_add_signed(by);
    }
}

/// What the card needs beyond the ask.
pub struct Card<'a> {
    pub key: &'a str,
    pub pane: PaneId,
    pub tokens: &'a Tokens,
    pub thread: &'a Thread,
    pub now: Millis,
    /// The document's card: the larger sizes, and Always says what it covers.
    pub wide: bool,
}

impl Card<'_> {
    fn sizes(&self) -> &'static Sizes {
        if self.wide { &WIDE } else { &PANE }
    }

    /// The card for the picked ask of `waiting`; none when nothing waits.
    pub fn view(&self, waiting: &[&Ask], pick: usize) -> Option<El> {
        let ask = *waiting.get(pick)?;
        let tokens = self.tokens;
        let call = gated(self.thread, ask);
        let label = kind(call, ask);
        let command = label == "Run a command";
        let card = format!("{}.asks.row.{}", self.key, ask.id.as_str());
        let sizes = self.sizes();
        Some(
            col()
                .key(card.clone())
                .w_full()
                .gap(sizes.gap)
                .pl(sizes.pad.0)
                .pr(sizes.pad.1)
                .py(sizes.pad.2)
                .radius(sizes.radius.unwrap_or(RADIUS_CARD.of(tokens)))
                .fill(colour(SOFT.of(tokens)))
                .border_sides([0.0, 0.0, 0.0, 3.0], colour(tokens.need))
                .focusable(true)
                .on_key({
                    let pane = self.pane;
                    move |win: &mut Win, event: &KeyEvent, _tx: &mut rune::Tx| key(win, pane, event)
                })
                .child(self.who(&card, ask, call, &label, waiting.len(), pick))
                .child(self.what(&card, ask, call, command))
                .child(
                    text(self.scope(ask))
                        .font(sizes.meta)
                        .color(colour(tokens.faint))
                        .wrap_anywhere(),
                ),
        )
    }

    fn who(
        &self,
        card: &str,
        ask: &Ask,
        call: Option<&ToolCall>,
        label: &str,
        of: usize,
        pick: usize,
    ) -> El {
        let tokens = self.tokens;
        let asker = call.and_then(|call| self.thread.asker(&call.id));
        let name = asker
            .map(|agent| line(&agent.update.name, 60))
            .unwrap_or_default();
        let why = asker
            .and_then(|agent| agent.update.task.as_deref())
            .map(|task| line(task, 120));
        let mut words = name.clone();
        let mut spans = vec![Span::new(0, name.len()).bold().color(colour(tokens.fg))];
        if let Some(why) = why {
            let start = words.len();
            words.push_str(&format!(" · {why}"));
            spans.push(Span::new(start, words.len()).color(colour(tokens.dim)));
        }
        let pane = self.pane;
        let sizes = self.sizes();
        let arrow = |glyph: &str, by: isize, part: &str| {
            text(glyph)
                .key(format!("{card}.{part}"))
                .font(sizes.meta)
                .color(colour(tokens.dim))
                .cursor(Cursor::Pointer)
                .on_press(move |win: &mut Win| step(win, pane, by))
        };
        row()
            .w_full()
            .h(sizes.head)
            .gap(7.0)
            .center_y()
            .child(thread_dot(tokens, Dot::Need, 8.0, true))
            .child(
                text(label)
                    .font(sizes.kind)
                    .color(colour(tokens.need))
                    .text_case(TextCase::Uppercase),
            )
            .child(
                rich(words, spans)
                    .font(sizes.meta)
                    .color(colour(tokens.dim))
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            )
            .child(
                text(waited(ask.at, self.now))
                    .font(sizes.meta)
                    .color(colour(tokens.faint)),
            )
            .children((of > 1).then(|| {
                row()
                    .gap(3.0)
                    .center_y()
                    .child(arrow("‹", -1, "prev"))
                    .child(
                        text(format!("{} of {of}", pick.saturating_add(1)))
                            .font(sizes.meta)
                            .color(colour(tokens.dim)),
                    )
                    .child(arrow("›", 1, "next"))
            }))
    }

    fn what(&self, card: &str, ask: &Ask, call: Option<&ToolCall>, command: bool) -> El {
        let tokens = self.tokens;
        let changes = ask.changes();
        let workdir = self.thread.workdir.as_ref().map(|w| w.cwd.as_str());
        let folder = workdir.unwrap_or(self.thread.cwd.as_str());
        let title = || (line(&ask.title, MAX_LINE), Read::Code(Lang::PLAIN), None);
        let (said, read, others) = match call {
            Some(call) if command || changes.is_empty() => {
                whole(call, folder).unwrap_or_else(title)
            }
            _ if !changes.is_empty() => (self.files(&changes), Read::Code(Lang::PLAIN), None),
            _ => title(),
        };
        let (shown, total) = clean(&said, MAX_COMMAND);
        let painted = paint(&shown, read);
        let prompt = if command { "$ " } else { "" };
        let words = format!("{prompt}{}", painted.text());
        let offset = u32::try_from(prompt.len()).unwrap_or(0);
        let coloured = painted.spans(tokens).into_iter().map(|mut span| {
            span.start = span.start.saturating_add(offset);
            span.end = span.end.saturating_add(offset);
            span
        });
        let mut spans = vec![Span::new(0, prompt.len()).color(colour(tokens.faint))];
        spans.extend(coloured);
        let lines = painted.text().trim_end().lines().count();
        let notices = [
            (total > MAX_COMMAND).then(|| format!("[…] {MAX_COMMAND} of {total} characters shown")),
            (lines > COMMAND_LINES)
                .then(|| format!("[…] {COMMAND_LINES} of {lines} lines in view; the rest scrolls")),
            painted.notice(),
        ];
        let below = others
            .into_iter()
            .chain(notices.into_iter().flatten())
            .map(|words| {
                text(words)
                    .font(META)
                    .color(colour(tokens.faint))
                    .wrap_anywhere()
            });
        let marked = ask.always();
        let buttons = ask.offers().map(|offer| {
            let (look, fixed, mut hint, mut part) = drawn(&offer);
            // Every Always but the one cmd+Return picks is keyed by its own id.
            if offer.kind == OptionKind::AllowAlways {
                let picked = marked == Some(offer.id.0);
                hint = picked.then_some("⌘↵");
                part = if picked { part } else { own_key(&offer) };
            }
            // With room to say it, Always keeps the server's words for what it will cover.
            let spelled = self.wide && offer.kind == OptionKind::AllowAlways;
            let word = match fixed {
                Some(fixed) if command && !spelled => fixed.to_owned(),
                _ => line(offer.name, 40),
            };
            let (id, option) = (ask.id.clone(), offer.id.0.to_owned());
            wrapping(tokens, look, &word, hint)
                .key(format!("{card}.{part}"))
                .on_press(move |win: &mut Win| answer_with(win, &id, &option))
        });
        let sizes = self.sizes();
        let well = row()
            .key(format!("{card}.command"))
            .grow(1.0)
            .min_w(COMMAND_MIN)
            .min_h(sizes.well.0)
            .px(sizes.well.1)
            .py((sizes.well.0 - sizes.command.line_height) / 2.0)
            .center_y()
            .radius(RADIUS_CHIP.of(tokens))
            .fill(colour(WELL.of(tokens)))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .max_h(sizes.command.line_height * COMMAND_LINES as f32)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(
                        rich(words, spans)
                            .font(sizes.command)
                            .color(colour(tokens.fg))
                            .wrap_anywhere(),
                    ),
            );
        // The mockup's rule: the buttons sit beside the command while both fit, else under it.
        let answer = row()
            .w_full()
            .min_w(0.0)
            .flex_wrap()
            .gaps(4.0, 6.0)
            .center_y()
            .child(well)
            .child(
                row()
                    .shrink()
                    .min_w(0.0)
                    .flex_wrap()
                    .gaps(4.0, 4.0)
                    .center_y()
                    .children(buttons),
            );
        col().w_full().gap(2.0).child(answer).children(below)
    }

    /// The files an edit would change, each under the folder the thread works in.
    fn files(&self, changes: &[Change]) -> String {
        let workdir = self.thread.workdir.as_ref().map(|w| w.cwd.as_str());
        let folder = workdir.unwrap_or(self.thread.cwd.as_str());
        let each = changes.iter().map(|change| {
            let path = change.path.as_str();
            // Yi edits in the thread's lane, `<home>/.yi/lanes/<lane>/<slot>/`, a copy of the folder.
            let lane = path.split_once("/.yi/lanes/").and_then(|(_, rest)| {
                let (_, rest) = rest.split_once('/')?;
                Some(rest.split_once('/')?.1)
            });
            let inside = path
                .strip_prefix(folder)
                .and_then(|rest| rest.strip_prefix('/'));
            let inside = lane.or(inside);
            let mut words = line(inside.unwrap_or(path), MAX_LINE);
            words.push_str(&format!(" +{}", change.added));
            if change.removed > 0 {
                words.push_str(&format!(" −{}", change.removed));
            }
            words
        });
        each.collect::<Vec<_>>().join(", ")
    }

    /// Where it runs and what the server says it may touch.
    fn scope(&self, ask: &Ask) -> String {
        let workdir = self.thread.workdir.as_ref().map(|w| w.cwd.as_str());
        let cwd = workdir.unwrap_or(self.thread.cwd.as_str());
        let mut words = if cwd.is_empty() {
            String::new()
        } else {
            format!("in {}", line(cwd, 120))
        };
        if let Some(said) = ask.description.as_deref() {
            if !words.is_empty() {
                words.push_str(" · ");
            }
            words.push_str(&line(said, MAX_LINE));
        }
        words
    }
}
