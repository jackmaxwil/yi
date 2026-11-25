//! The reading column and its bottom stack: the whole window as one conversation that reads
//! like a document (record 3.1), which is also what the Just chat layout shows (3.6).

pub(crate) mod scene;
#[cfg(test)]
mod tests;

use rune::{Cx, El, Icon, ScrollSpec, TextStyle, col, el, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use yd_core::ids::{Millis, PaneId};
use yd_core::model::Model;
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::{Message, Turn};
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::icons::glyph;
use yd_ui::line_icon;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::colour;
use yd_ui::theme::look::{BODY, LABEL, READ_PROSE, READ_QUOTE, READ_SMALL, READ_TITLE, TITLE};
use yd_ui::theme::scale::{LINE_BAR, LINE_QUIET, LINE_SOFT, RADIUS_CARET, SEMIBOLD};
use yd_ui::win::{Hub, Win};

use crate::chat::approval::{Card, queue};
use crate::chat::composer::{Composer, Form, Send, drawn};
use crate::chat::messages::{Block as Part, MAX_FOLDED, clean, clock, line, model_name};
use crate::chat::pane::{Chat, chat_mut, jump};
use crate::chat_doc::hover::{Block, Hover, Said, cost, over};
use crate::chat_doc::steps::{anchor, calls, card, changed, popover, summary};
use crate::chat_doc::turn_map;
use yd_ui::states::status::{Status, bar, body};

use crate::copy::empty::{SERVER, THREADLESS, down, threadless};

pub static CHEVRON: Icon = line_icon!("chevron", r#"<path d="M9 6l6 6-6 6"/>"#);

/// The reading column, in both forms: record 9.1 found the mockup using two widths.
pub const WIDTH: f32 = 770.0;
/// The column's side padding: content is 726 wide.
pub const SIDE: f32 = 22.0;
const MAX_TEXT: usize = 20_000;

/// How the details are shown: as the pointer finds them, or all at once (the mockup's pose).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pose {
    #[default]
    Pointer,
    Every,
}

/// What one document remembers: the block the pointer rests on, past the intent delay.
#[derive(Debug, Default)]
pub struct Doc {
    pub hover: Option<Block>,
    /// The pointer is on the steps pop-over, which keeps it open.
    pub over: bool,
    pub pose: Pose,
}

impl Doc {
    pub fn shows(&self, block: Block) -> bool {
        self.pose == Pose::Every || self.hover == Some(block)
    }
}

pub const SCENES: &[Scene] = &[
    Scene {
        name: "chat-doc/mockup",
        stage: Stage::View(page),
        setup: scene::mockup,
    },
    Scene {
        name: "chat-doc/hover",
        stage: Stage::View(page),
        setup: scene::hover,
    },
    Scene {
        name: "chat-doc/empty",
        stage: Stage::View(page),
        setup: |_| {},
    },
];

/// `3.4 s`, or `9 s` when the tenths are nought.
pub fn seconds(ms: u64) -> String {
    let (whole, tenth) = (ms / 1000, (ms % 1000) / 100);
    match tenth {
        0 => format!("{whole} s"),
        _ => format!("{whole}.{tenth} s"),
    }
}

/// A span of work in words: `14 seconds`, `1 second`, `3m 20s`.
pub fn long(ms: u64) -> String {
    match ms / 1000 {
        1 => "1 second".to_owned(),
        s @ 0..60 => format!("{s} seconds"),
        s => format!("{}m {}s", s / 60, s % 60),
    }
}

/// How long ago `at` was: `now` under a minute, then minutes, hours, days.
pub fn ago(at: Millis, now: Millis) -> String {
    match now.0.saturating_sub(at.0) / 60_000 {
        0 => "now".to_owned(),
        m @ 1..60 => format!("{m}m ago"),
        m @ 60..1440 => format!("{}h ago", m / 60),
        m => format!("{}d ago", m / 1440),
    }
}

/// The messages of a turn, each with its index in the transcript.
fn messages(thread: &Thread, turn: Turn) -> impl Iterator<Item = (usize, &Message)> {
    let all = &thread.transcript.messages;
    let range = all.get(turn.first..).unwrap_or_default();
    range
        .iter()
        .take(turn.len)
        .enumerate()
        .map(move |(n, message)| (turn.first.saturating_add(n), message))
}

/// The words a message says, its text blocks joined.
fn words(message: &Message) -> Option<(bool, String)> {
    let said = message.said()?;
    let texts: Vec<String> = said
        .blocks
        .into_iter()
        .filter_map(|block| match block {
            Part::Text(words) => Some(words),
            Part::Thinking(_) | Part::Call(_) => None,
        })
        .collect();
    Some((said.user, texts.join("\n\n")))
}

fn turn_cost(thread: &Thread, turn: Turn) -> f64 {
    messages(thread, turn).map(|(_, m)| cost(m)).sum()
}

/// How long the turn worked: up to the ask that holds it, else its end, else now.
fn work(model: &Model, thread: &Thread, turn: Turn) -> u64 {
    let held =
        model.wire.asks.iter().filter(|ask| {
            ask.thread == thread.id && ask.answer.is_none() && ask.at >= turn.started
        });
    let until = held
        .map(|ask| ask.at)
        .min()
        .or(turn.ended)
        .unwrap_or(model.now);
    until.0.saturating_sub(turn.started.0)
}

/// The window's title and the current turn's number, age and cost (record 3.1.2).
pub fn meta(thread: &Thread, now: Millis) -> String {
    let turns = thread.transcript.turns();
    let Some(turn) = turns.last().copied() else {
        return String::new();
    };
    let minutes = now.0.saturating_sub(turn.started.0) / 60_000;
    let spent = turn_cost(thread, turn);
    format!("turn {} · {minutes}m · ${spent:.2}", turn.number)
}

/// The title where the top bar was: plan default 12, chat only.
pub fn top(tokens: &Tokens, title: &str, meta: &str) -> El {
    row()
        .key("window.title")
        .w_full()
        .h(30.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .justify_center()
        .window_drag()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(LINE_BAR.of(tokens)))
        .child(
            text(title)
                .font(TITLE)
                .color(colour(tokens.fg))
                .truncate()
                .min_w(0.0),
        )
        .children((!meta.is_empty()).then(|| text(meta).font(LABEL).color(colour(tokens.faint))))
}

/// The scene's window: the document of the pane every scene shows alone.
fn page(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    page_of(win, hub, cx, SCENE_PANE)
}

/// The window as one conversation, which is Just chat: its title where the top bar was,
/// then the document of chat pane `pane`, whose draft and picked ask it shares.
pub fn page_of(win: &Win, hub: &Hub, cx: &mut Cx<Win>, pane: PaneId) -> El {
    let pane = PaneCx {
        hub,
        pane,
        focused: true,
        edits: Some(&win.edits),
    };
    let tokens = pane.tokens();
    let (title, words) = pane.thread().map_or_else(
        || ("Chat".to_owned(), String::new()),
        |t| {
            let title = t.title.clone().unwrap_or_else(|| "Untitled".to_owned());
            (title, meta(t, hub.model.now))
        },
    );
    col()
        .w_full()
        .h_full()
        .child(top(tokens, &title, &words))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(document(&pane, cx)),
        )
}

/// Markdown at the reading size: 13.5 on 21.6, inline code 12 mono on the raised fill.
fn prose(tokens: &Tokens, words: &str, streaming: bool, moving: bool) -> El {
    let (shown, _) = clean(words, MAX_TEXT);
    let style = MarkdownStyle::neutral(true)
        .body_font(READ_PROSE)
        .text(colour(tokens.fg))
        .inline_code_bg(colour(tokens.raised))
        .inline_code_font(12.0 / 13.5, 400.0)
        .list(18.0, 2.0);
    let plain = shown.replace('`', "");
    let body = Markdown::new(shown)
        .streaming(streaming)
        .selectable(true)
        .render(&style);
    let caret = streaming.then(|| {
        let caret = el()
            .size(6.0, 12.0)
            .radius(RADIUS_CARET.of(tokens))
            .fill(colour(tokens.accent));
        if moving {
            caret.cycle(rune::Cycle {
                period: 1.0,
                phase: 0.0,
                stops: &[(0.0, 1.0), (0.5, 1.0), (0.5, 0.0), (1.0, 0.0)],
                still: 1.0,
                fps: 4.0,
            })
        } else {
            caret
        }
    });
    col()
        .label(plain)
        .w_full()
        .gap(2.0)
        .child(body)
        .children(caret)
}

/// Text split at its headings: the lead, then each heading with what follows it.
fn sections(words: &str) -> (String, Vec<(String, String)>) {
    let mut lead = String::new();
    let mut rest: Vec<(String, String)> = Vec::new();
    for line in words.lines() {
        if let Some(heading) = line.trim_start().strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim().to_owned();
            rest.push((heading, String::new()));
            continue;
        }
        let into = match rest.last_mut() {
            Some((_, body)) => body,
            None => &mut lead,
        };
        into.push_str(line);
        into.push('\n');
    }
    (lead.trim().to_owned(), rest)
}

fn heading(tokens: &Tokens, words: &str) -> El {
    text(words.to_owned())
        .font(TextStyle::new(12.0, SEMIBOLD))
        .color(colour(tokens.dim))
        .mt(2.0)
}

/// What the document draws from; one per view run.
struct Page<'a> {
    hover: Hover<'a>,
    model: &'a Model,
    thread: &'a Thread,
    moving: bool,
}

impl Page<'_> {
    fn tokens(&self) -> &Tokens {
        self.hover.tokens
    }

    fn said(&self, words: &str, message: &Message) -> Said {
        Said {
            words: words.to_owned(),
            thread: self.thread.id.clone(),
            entry: message.entry.clone(),
        }
    }

    /// An earlier turn as one line: its number, what it found, and when.
    fn folded(&self, turn: Turn) -> El {
        let tokens = self.tokens();
        let answer = messages(self.thread, turn)
            .filter_map(|(_, m)| words(m))
            .filter(|(user, words)| !user && !words.trim().is_empty())
            .last()
            .map(|(_, words)| line(&words, 120))
            .unwrap_or_default();
        let span = turn
            .ended
            .map_or(0, |end| end.0.saturating_sub(turn.started.0));
        let facts = [
            clock(turn.started),
            format!("{} steps · {}", turn.steps, seconds(span)),
            format!("${:.2}", turn_cost(self.thread, turn)),
        ];
        let body = row()
            .w_full()
            .h(22.0)
            .gap(8.0)
            .center_y()
            .child(glyph(&CHEVRON, 11.0, tokens.faint))
            .child(
                text(format!("Turn {}", turn.number))
                    .font(TextStyle::new(12.0, SEMIBOLD))
                    .color(colour(tokens.dim)),
            )
            .child(
                text(answer)
                    .font(READ_SMALL)
                    .color(colour(tokens.faint))
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            )
            .child(
                text(ago(turn.started, self.model.now))
                    .font(READ_SMALL)
                    .color(colour(tokens.faint)),
            );
        self.hover
            .block(Block::Turn(turn.number), &facts, None, body)
    }

    /// The current turn: its title, the ask, the steps line and the answers.
    fn current(&self, turn: Turn, out: &mut Vec<El>) -> Option<El> {
        let tokens = self.tokens();
        let key = self.hover.key;
        let named = self
            .thread
            .work
            .plan
            .as_ref()
            .and_then(|p| p.running.as_deref());
        let title = match named {
            Some(name) => format!("Turn {} · {}", turn.number, line(name, 120)),
            None => format!("Turn {}", turn.number),
        };
        let worked = work(self.model, self.thread, turn);
        let when = format!(
            "{} · {} of work",
            ago(turn.started, self.model.now),
            long(worked)
        );
        out.push(
            row()
                .key(format!("{key}.title"))
                .w_full()
                .mt(2.0)
                .pt(6.0)
                .gap(10.0)
                .center_y()
                .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_SOFT.of(tokens)))
                .child(text(title).font(READ_TITLE).color(colour(tokens.fg)))
                .child(text(when).font(BODY).color(colour(tokens.faint))),
        );
        let steps = calls(self.thread, turn);
        let (edits, files, added, removed) = changed(&steps);
        let last = messages(self.thread, turn)
            .filter(|(_, m)| m.done.is_some() && words(m).is_some_and(|(user, _)| !user))
            .map(|(at, _)| at)
            .last();
        for (at, message) in messages(self.thread, turn) {
            let Some((user, said)) = words(message) else {
                continue;
            };
            if user {
                out.push(self.ask(message, &said));
                if at == turn.first && !steps.is_empty() {
                    out.push(self.steps(&steps, worked));
                }
                continue;
            }
            if said.trim().is_empty() {
                continue;
            }
            let Some(done) = message.done else {
                out.push(self.writing(message, &said));
                continue;
            };
            let (lead, rest) = sections(&said);
            out.push(self.answer(turn, at, message, done, &lead));
            let closing = Some(at) == last && files > 0;
            let count = rest.len();
            for (n, (title, body)) in rest.into_iter().enumerate() {
                out.push(heading(tokens, &title));
                if closing && n.saturating_add(1) == count {
                    out.push(self.changes(message, &body, &edits, files, added, removed));
                } else {
                    out.push(prose(tokens, &body, false, false));
                }
            }
            if closing && count == 0 {
                out.push(self.changes(message, "", &edits, files, added, removed));
            }
        }
        let open = self
            .hover
            .doc
            .is_some_and(|d| d.shows(Block::Steps) || d.over);
        (open && !steps.is_empty()).then(|| {
            let pane = self.hover.pane;
            popover(
                key,
                tokens,
                &steps,
                &format!("{key}.steps-anchor"),
                move |win, inside| over(win, pane, inside),
            )
        })
    }

    fn ask(&self, message: &Message, said: &str) -> El {
        let tokens = self.tokens();
        let attached = said
            .split_whitespace()
            .filter_map(|word| word.parse::<yd_core::address::uri::Address>().ok())
            .collect::<Vec<_>>();
        let typed = said.split(" yi://").next().unwrap_or_default().trim();
        let (shown, _) = clean(typed, MAX_TEXT);
        let mut facts = vec![clock(message.at), "you".to_owned()];
        if !attached.is_empty() {
            let files = attached.iter().all(|a| {
                matches!(
                    a.kind(),
                    yd_core::kind::Kind::File | yd_core::kind::Kind::Lines
                )
            });
            let noun = if files { "files" } else { "things" };
            facts.push(format!("{} {noun} in context", attached.len()));
        }
        let body = col()
            .w_full()
            .pl(12.0)
            .border_sides([0.0, 0.0, 0.0, 2.0], colour(LINE_QUIET.of(tokens)))
            .child(
                text(shown.clone())
                    .font(READ_QUOTE)
                    .color(colour(tokens.dim))
                    .wrap_anywhere()
                    .selectable(true)
                    .selection_color(a11y::selection(tokens)),
            );
        let said = self.said(&shown, message);
        self.hover.block(Block::Ask, &facts, Some(said), body)
    }

    fn steps(&self, steps: &[&yd_core::reduce::turns::ToolCall], worked: u64) -> El {
        let first = steps.iter().filter_map(|c| c.started).min();
        let end = steps.iter().filter_map(|c| c.ended).max();
        let n = steps.len();
        let facts = [
            first.map(clock).unwrap_or_default(),
            format!("to {}", end.map(clock).unwrap_or_default()),
            format!("{n} steps"),
        ];
        let lit = self
            .hover
            .doc
            .is_some_and(|d| d.shows(Block::Steps) || d.over);
        let words = format!("Worked for {} · {n} steps", long(worked));
        let body = col()
            .w_full()
            .child(summary(self.tokens(), words, lit))
            .child(anchor(&format!("{}.steps-anchor", self.hover.key)));
        self.hover.block(Block::Steps, &facts, None, body)
    }

    fn answer(&self, turn: Turn, at: usize, message: &Message, done: Millis, lead: &str) -> El {
        let output = message.said().map_or(0, |s| s.output);
        let model = message.said().map(|s| model_name(self.thread, &s.model));
        let effort = self.thread.option("thought_level").and_then(|o| {
            let choice = o.options.iter().find(|c| c.value == o.current_value);
            choice.map(|c| c.name.clone())
        });
        let named = match (model, effort) {
            (Some(model), Some(effort)) => format!("{model} · {effort}"),
            (Some(model), None) => model,
            (None, _) => String::new(),
        };
        let facts = [
            clock(done),
            named,
            format!(
                "{} · {output} tokens",
                seconds(done.0.saturating_sub(turn.started.0))
            ),
        ];
        let body = prose(self.tokens(), lead, false, false);
        let said = self.said(lead, message);
        self.hover
            .block(Block::Answer(at), &facts, Some(said), body)
    }

    fn changes(
        &self,
        message: &Message,
        body: &str,
        edits: &[&yd_core::reduce::turns::ToolCall],
        files: usize,
        added: u64,
        removed: u64,
    ) -> El {
        let tokens = self.tokens();
        let noun = if files == 1 { "file" } else { "files" };
        let mut counts = format!("+{added}");
        if removed > 0 {
            counts.push_str(&format!(" −{removed}"));
        }
        let facts = [format!("{files} {noun}"), counts];
        let shown = edits.first().and_then(|call| {
            let key = format!("{}.card.{}", self.hover.key, call.id.as_str());
            card(&key, tokens, call)
        });
        let content = col()
            .w_full()
            .gap(10.0)
            .children((!body.trim().is_empty()).then(|| prose(tokens, body, false, false)))
            .children(shown);
        let said = self.said(body, message);
        self.hover
            .block(Block::Changes, &facts, Some(said), content)
    }

    fn writing(&self, message: &Message, said: &str) -> El {
        let output = message.said().map_or(0, |s| s.output);
        let secs = self.model.now.0.saturating_sub(message.at.0) / 1000;
        let mut facts = vec![ago(message.at, self.model.now), "writing".to_owned()];
        if let Some(rate) = output.checked_div(secs) {
            facts.push(format!("{rate} tokens a second"));
        }
        let body = prose(self.tokens(), said, true, self.moving);
        self.hover.block(Block::Writing, &facts, None, body)
    }
}

/// The conversation as a document for the thread `pane`'s window shows: what the full-screen
/// chat and the Just chat layout draw.
pub fn document(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Chat);
    let tokens = pane.tokens();
    let Some(thread) = pane.thread() else {
        let (status, words) = threadless(pane).unwrap_or((Status::Empty, &THREADLESS));
        return body(pane, PaneKind::Chat, &status, words);
    };
    let model = pane.model();
    let page = Page {
        hover: Hover {
            key: &key,
            pane: pane.pane,
            tokens,
            doc: pane.state::<Doc>(),
        },
        model,
        thread,
        moving: !cx.is_reduced_motion(),
    };
    let turns = thread.transcript.turns();
    let (earlier, current) = match turns.split_last() {
        Some((last, earlier)) => (earlier, Some(*last)),
        None => (&[][..], None),
    };
    let mut blocks = Vec::new();
    let hidden = earlier.len().saturating_sub(MAX_FOLDED);
    if hidden > 0 {
        blocks.push(
            text(format!(
                "[…] {MAX_FOLDED} of {} earlier turns folded here; the thread's history shows the rest",
                earlier.len()
            ))
            .font(READ_SMALL)
            .color(colour(tokens.faint)),
        );
    }
    blocks.extend(earlier.iter().skip(hidden).map(|turn| page.folded(*turn)));
    let popover = current.and_then(|turn| page.current(turn, &mut blocks));
    let pick = pane.state::<Chat>().map_or(0, |chat| chat.pick);
    let (waiting, pick) = queue(&model.wire.asks, thread, pick);
    let card = Card {
        key: &key,
        pane: pane.pane,
        tokens,
        thread,
        now: model.now,
        wide: true,
    }
    .view(&waiting, pick);
    let composer = Composer {
        key: &key,
        pane: pane.pane,
        tokens,
        thread: Some(thread),
        draft: drawn(pane),
        queued: model.wire.queued(&thread.id).collect(),
        held: model.wire.holds(&thread.id),
        files: model.tables.get(yd_core::feed::FILES),
        hint: "Reply, or ask for a change to any section",
        form: Form::Reading { gap: 10.0 },
        send: Send::Prompt,
    };
    let entries = turn_map::entries(&model.wire, thread, &turns);
    let (id, away) = (
        pane.pane,
        pane.state::<Chat>().is_some_and(|chat| chat.away),
    );
    let column = |child: El| row().w_full().justify_center().child(child);
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .child(
            col()
                .key(format!("{key}.doc"))
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .on_at_bottom(move |win: &mut Win, at_tail| {
                    chat_mut(win, id, |chat| chat.away = !at_tail);
                })
                .child(column(
                    col()
                        .key(format!("{key}.chat-column"))
                        .w(WIDTH)
                        .px(SIDE)
                        .pt(16.0)
                        .pb(10.0)
                        .gap(10.0)
                        .children(blocks),
                )),
        )
        .children(jump(tokens, &key, id, format!("{key}.doc"), away))
        .child(column(
            col()
                .w(WIDTH)
                .px(SIDE)
                .pb(16.0)
                .gap(10.0)
                .children(down(pane).map(|down| bar(pane, &key, &down, &SERVER)))
                .children(crate::chat::tray::view(pane))
                .children(card)
                .child(composer.view()),
        ))
        .child(
            row()
                .pin(Some(0.0), Some(58.0), Some(0.0), None)
                .justify_center()
                .child(
                    col()
                        .w(WIDTH)
                        .h(0.0)
                        .child(turn_map::map(&key, tokens, &entries).absolute(WIDTH + 26.0, 0.0)),
                ),
        )
        .children(popover)
}
