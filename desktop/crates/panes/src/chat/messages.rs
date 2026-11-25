//! Streaming answers and the user's messages.

use std::cell::RefCell;
use std::ops::Range;

use rune::{Cursor, Cycle, El, col, el, memo, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use yd_core::address::uri::Address;
use yd_core::ids::{Millis, PaneId};
use yd_core::kind::Kind;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::{Message, Transcript, Turn};
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::theme::look::{BODY, META, colour};
use yd_ui::theme::scale::RADIUS_CARET;

use crate::chat::pane::chat_mut;
use crate::chat::pane::child::spawned;
use crate::chat::tools;
use yd_ui::win::Win;

/// Characters one message may draw; the rest is named in a notice where it is cut.
pub const MAX_CHARS: usize = 20_000;
/// Messages of the open turn drawn; older ones are named in a notice.
pub const MAX_SHOWN: usize = 200;
/// Earlier turns drawn as one folded line each.
pub const MAX_FOLDED: usize = 50;
/// Seconds a word of a streaming answer takes to fade in: the framework's feathered edge.
const WORD_FADE: f32 = 0.18;

// Incident: this crate cannot name yi-types or serde_json, so a message reaches plain JSON
// through Rune's in-memory JSON store; a `ToolCall` only lends its JSON-typed field.
macro_rules! plain {
    ($value:expr) => {{
        let mut store = rune::prefs::Prefs::memory();
        let mut holder = yd_core::reduce::turns::ToolCall::default();
        if store.set("v", $value).is_ok() {
            holder.args = store.get("v").unwrap_or_default();
        }
        holder.args
    }};
}
pub(crate) use plain;

pub use yd_core::reduce::said::{Block, Said};

/// What a message says; none for a kind the pane does not show.
pub fn said(message: &Message) -> Option<Said> {
    message.said()
}

/// `text` with control and direction marks taken out, cut at `cap` characters; the second
/// value is how many characters there were.
pub fn clean(text: &str, cap: usize) -> (String, usize) {
    let bidi = |c: char| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let kept = text
        .chars()
        .filter(|&c| (!c.is_control() || c == '\n') && !bidi(c));
    let mut out = String::new();
    let mut total = 0usize;
    for c in kept {
        if total < cap {
            out.push(c);
        }
        total = total.saturating_add(1);
    }
    (out, total)
}

/// One line of text, cleaned and cut at `cap` characters with an ellipsis.
pub fn line(text: &str, cap: usize) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let (mut out, total) = clean(first, cap);
    if total > cap {
        out.push('…');
    }
    out
}

/// `HH:MM:SS` of a time.
// ponytail: UTC; the local offset comes with start-up, which knows the zone.
pub fn clock(at: Millis) -> String {
    let secs = at.0 / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// A span of work: `3.4 s` under a minute, `2m 5s` past it.
pub fn span(ms: u64) -> String {
    if ms < 60_000 {
        format!("{}.{} s", ms / 1000, (ms % 1000) / 100)
    } else {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// A token count as the pane header says it: `612`, `41.2K`, `1.3M`.
pub fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}.{}K", n / 1000, (n % 1000) / 100),
        _ => format!("{}.{}M", n / 1_000_000, (n % 1_000_000) / 100_000),
    }
}

/// The name the server gives a model id among the thread's options, else the id.
pub fn model_name(thread: &Thread, id: &str) -> String {
    let option = thread.option("model");
    let choices = option.map(|o| o.options.as_slice()).unwrap_or_default();
    let short = id.rsplit('/').next().unwrap_or(id);
    let named = choices
        .iter()
        .find(|c| c.value == id || c.value.rsplit('/').next() == Some(short));
    named.map_or_else(|| short.to_owned(), |c| c.name.clone())
}

/// What the pane draws of a thread: earlier turns folded, the open turn whole.
pub struct Frame<'a> {
    pub key: &'a str,
    pub pane: PaneId,
    /// Earlier turns the reader opened, by number.
    pub opened: &'a [u32],
    /// The folder the thread works in, where its commands ran.
    pub folder: &'a str,
    /// Steps the reader opened.
    pub steps: &'a [yd_core::reduce::turns::CallId],
    /// Prompts the thread has not taken yet.
    pub queued: &'a [&'a str],
    pub tokens: &'a Tokens,
    pub thread: &'a Thread,
    /// What is drawn: the thread's own turns, or one child agent's.
    pub transcript: &'a Transcript,
    pub now: Millis,
    /// An ask holds this thread up.
    pub held: bool,
    pub moving: bool,
    /// The theme's stamp, which a kept answer is drawn again under.
    pub stamp: u64,
    /// The names of the models met in this frame, each looked up once.
    pub names: RefCell<Vec<(String, String)>>,
}

impl Frame<'_> {
    pub fn conversation(&self) -> Vec<El> {
        let all = &self.transcript.messages;
        let turns = self.transcript.turns();
        let (earlier, open) = match turns.split_last() {
            Some((last, earlier)) => (earlier, Some(*last)),
            None => (&[][..], None),
        };
        let mut out = Vec::new();
        let hidden = earlier.len().saturating_sub(MAX_FOLDED);
        if hidden > 0 {
            out.push(self.notice(&format!(
                "[…] {} of {} earlier turns folded here; the thread's history shows the rest",
                MAX_FOLDED,
                earlier.len()
            )));
        }
        for turn in earlier.iter().skip(hidden) {
            out.push(self.folded(turn));
            if self.opened.contains(&turn.number) {
                let end = turn.first.saturating_add(turn.len);
                self.range(Some(*turn), turn.first..end, 0, &mut out);
            }
        }
        let first = open.map_or(0, |turn| turn.first);
        let shown = all.get(first..).unwrap_or_default();
        let skip = shown.len().saturating_sub(MAX_SHOWN);
        if skip > 0 {
            out.push(self.notice(&format!(
                "[…] {MAX_SHOWN} of {} messages of this turn shown (newest); scroll the thread's history for the rest",
                shown.len()
            )));
        }
        self.range(open, first..all.len(), skip, &mut out);
        out
    }

    /// Incident: each answer read its whole turn again to count changed files, 3,600 reads
    /// a frame for sixty answers. A turn's messages at `range` are read once, from `skip` on.
    fn range(&self, turn: Option<Turn>, range: Range<usize>, skip: usize, out: &mut Vec<El>) {
        let messages = self.transcript.messages.get(range.clone());
        let messages = messages.unwrap_or_default();
        let read: Vec<Option<Said>> = messages.iter().map(said).collect();
        let files = turn.map_or(0, |_| self.changed(&read));
        // Record 3.4.3: tool rows sit one under the next, 20 apart, whichever message of the
        // turn called them; only what is said between them takes the conversation's gap.
        let mut steps = Vec::new();
        for (index, (message, said)) in messages.iter().zip(&read).enumerate().skip(skip) {
            let Some(said) = said else { continue };
            let at = Place {
                at: range.start.saturating_add(index),
                turn,
                files,
            };
            self.message(message, said, at, &mut steps, out);
        }
        flush(&mut steps, out);
    }

    fn notice(&self, words: &str) -> El {
        text(words)
            .font(META)
            .color(colour(self.tokens.faint))
            .wrap()
    }

    /// A turn before the open one, as one line: what was asked, its steps and its time. A
    /// press, or Return on it, opens it in place and closes it again.
    fn folded(&self, turn: &Turn) -> El {
        let all = self.transcript.messages.get(turn.first..);
        let range = all.unwrap_or_default().iter().take(turn.len);
        // The turn's prompt opens it; its steps are counted where it is folded from the wire.
        let ended = range.clone().map(|m| m.done.unwrap_or(m.at)).max();
        let ended = ended.unwrap_or(turn.started).max(turn.started);
        let prompt = range.clone().next().and_then(said).filter(|said| said.user);
        let typed = prompt
            .iter()
            .flat_map(|said| &said.blocks)
            .find_map(|b| match b {
                Block::Text(words) => {
                    Some(line(words.split(" yi://").next().unwrap_or_default(), 120))
                }
                Block::Thinking(_) | Block::Call(_) => None,
            });
        let asked = typed.unwrap_or_default();
        let steps = turn.steps;
        let mut words = format!("Turn {} · {asked}", turn.number);
        match steps {
            0 => {}
            1 => words.push_str(" · 1 step"),
            n => words.push_str(&format!(" · {n} steps")),
        }
        words.push_str(&format!(
            " · {}",
            span(ended.0.saturating_sub(turn.started.0))
        ));
        let open = self.opened.contains(&turn.number);
        let (pane, number) = (self.pane, turn.number);
        row()
            .key(format!("{}.turn.{}", self.key, turn.number))
            .w_full()
            .h(18.0)
            .gap(5.0)
            .center_y()
            .cursor(Cursor::Pointer)
            .label(format!("{} {words}", if open { "Close" } else { "Open" }))
            .on_press(move |win: &mut Win| {
                chat_mut(win, pane, |chat| {
                    match chat.open.iter().position(|n| *n == number) {
                        Some(at) => {
                            chat.open.remove(at);
                        }
                        None => chat.open.push(number),
                    }
                });
            })
            .child(
                text(if open { "▾" } else { "▸" })
                    .font(META)
                    .color(colour(self.tokens.faint)),
            )
            .child(
                text(words)
                    .font(BODY)
                    .color(colour(self.tokens.faint))
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            )
    }

    /// Draws `message`, read as `said`, into `out`; its tool rows wait in `steps` for the
    /// rows that follow.
    fn message(
        &self,
        message: &Message,
        said: &Said,
        place: Place,
        steps: &mut Vec<El>,
        out: &mut Vec<El>,
    ) {
        let key = format!("{}.message.{}", self.key, place.at);
        if said.user {
            flush(steps, out);
            out.extend(self.user(&key, message, said));
            return;
        }
        if said.stopped {
            flush(steps, out);
            out.push(
                text("Stopped")
                    .key(format!("{key}.stopped"))
                    .font(META)
                    .color(colour(self.tokens.warn)),
            );
        }
        for block in &said.blocks {
            match block {
                Block::Text(words) => {
                    flush(steps, out);
                    out.push(self.answer(&key, words, message.done.is_none()));
                }
                Block::Thinking(words) => {
                    flush(steps, out);
                    out.push(
                        text(format!("Thought · {}", line(words, 160)))
                            .font(BODY)
                            .color(colour(self.tokens.faint))
                            .truncate(),
                    );
                }
                Block::Call(id) => {
                    let call = self.transcript.tools.get(id.as_str());
                    steps.extend(call.map(|call| tools::step(self, call)));
                    steps.extend(spawned(self.key, self.tokens, self.thread, id));
                    steps.extend(crate::ipython::pane::link(
                        self.key,
                        self.tokens,
                        self.thread,
                        id,
                    ));
                }
            }
        }
        let wrote = said.blocks.iter().any(|b| matches!(b, Block::Text(_)));
        if wrote {
            out.push(self.meta(&key, message, said, place));
        }
    }

    fn user(&self, key: &str, message: &Message, said: &Said) -> Vec<El> {
        let words: Vec<&str> = said
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.as_str()),
                Block::Thinking(_) | Block::Call(_) => None,
            })
            .collect();
        let whole = words.join("");
        // The server stores what was attached as addresses after the words typed.
        let attached = whole
            .split_whitespace()
            .filter_map(|word| word.parse::<Address>().ok())
            .collect::<Vec<_>>();
        let typed = whole.split(" yi://").next().unwrap_or_default().trim();
        let (shown, total) = clean(typed, MAX_CHARS);
        let mut meta = format!("you · {}", clock(message.at));
        if !attached.is_empty() {
            let files = attached
                .iter()
                .all(|a| matches!(a.kind(), Kind::File | Kind::Lines));
            let noun = if files { "files" } else { "things" };
            meta.push_str(&format!(" · {} {noun} in context", attached.len()));
        }
        let bubble = row().w_full().justify_end().pl(24.0).child(
            col()
                .key(format!("{key}.bubble"))
                .shrink()
                .min_w(0.0)
                .px(9.0)
                .py(6.0)
                .radius(10.0)
                .fill(colour(self.tokens.raised))
                .child(
                    text(shown)
                        .font(BODY)
                        .color(colour(self.tokens.fg))
                        .wrap_anywhere()
                        .selectable(true)
                        .selection_color(a11y::selection(self.tokens)),
                ),
        );
        let mut out = vec![bubble];
        if total > MAX_CHARS {
            out.push(self.cut(total));
        }
        out.push(
            row().w_full().justify_end().child(
                text(meta)
                    .font(META)
                    .color(colour(self.tokens.faint))
                    .truncate(),
            ),
        );
        out
    }

    fn cut(&self, total: usize) -> El {
        self.notice(&format!(
            "[…] {MAX_CHARS} of {total} characters drawn (the message cap); copy the message for the rest"
        ))
    }

    /// Incident: finished answers were parsed again every frame, 9 ms a keystroke for sixty.
    /// A whole answer is kept until its words or the theme change.
    fn answer(&self, key: &str, words: &str, streaming: bool) -> El {
        if streaming {
            return self.written(key, words, true);
        }
        let kept = (words, self.stamp);
        memo(format!("{key}.answer"), kept, || {
            self.written(key, words, false)
        })
    }

    fn written(&self, key: &str, words: &str, streaming: bool) -> El {
        let (shown, total) = clean(words, MAX_CHARS);
        let style = MarkdownStyle::neutral(true).body_font(BODY.lh(17.25));
        // Markdown sets inline code between direction isolates; the label is the plain words.
        let words = shown.replace('`', "");
        // Words fade in as they arrive; under reduced motion they are simply there.
        let fade = if streaming && self.moving {
            WORD_FADE
        } else {
            0.0
        };
        let body = Markdown::new(shown)
            .streaming(streaming)
            .reveal(fade)
            .selectable(true)
            .render(&style);
        let mut answer = col()
            .key(format!("{key}.answer"))
            .label(words)
            .w_full()
            .gap(2.0)
            .streaming(streaming)
            .child(body);
        if total > MAX_CHARS {
            answer = answer.child(self.cut(total));
        }
        if streaming {
            answer = answer.child(self.caret());
        }
        answer
    }

    /// The working caret: it blinks while the answer streams, and holds still under reduced
    /// motion.
    fn caret(&self) -> El {
        const BLINK: Cycle = Cycle {
            period: 1.0,
            phase: 0.0,
            stops: &[(0.0, 1.0), (0.5, 1.0), (0.5, 0.0), (1.0, 0.0)],
            still: 1.0,
            fps: 4.0,
        };
        let caret = el()
            .size(6.0, 12.0)
            .radius(RADIUS_CARET.of(self.tokens))
            .fill(colour(self.tokens.accent));
        if self.moving {
            caret.cycle(BLINK)
        } else {
            caret
        }
    }

    fn meta(&self, key: &str, message: &Message, said: &Said, place: Place) -> El {
        let words = match message.done {
            None => {
                let secs = self.now.0.saturating_sub(message.at.0) / 1000;
                let mut words = String::from("writing");
                if let Some(rate) = said.output.checked_div(secs) {
                    words.push_str(&format!(" · {rate} tokens a second"));
                }
                if self.held {
                    words.push_str(" · held for your approval");
                }
                words
            }
            Some(done) => {
                let started = place.turn.map_or(message.at, |turn| turn.started);
                // A provider that reports no usage gives zero, which is not what was spent.
                let spent = match said.output {
                    0 => "no usage reported".to_owned(),
                    n => format!("{n} tokens"),
                };
                let mut words = format!(
                    "{} · {} · {} · {spent}",
                    self.named(&said.model),
                    clock(done),
                    span(done.0.saturating_sub(started.0)),
                );
                match place.files {
                    0 => {}
                    1 => words.push_str(" · 1 file changed"),
                    n => words.push_str(&format!(" · {n} files changed")),
                }
                words
            }
        };
        let tint = colour(self.tokens.faint);
        text(words)
            .key(format!("{key}.meta"))
            .font(META.lh(13.0))
            .color(tint)
            .truncate()
    }

    /// [`model_name`] of `id`. Incident: every answer looked its model up among the thread's
    /// choices, a hundred of them split at each slash, which took a fifth of a long frame.
    fn named(&self, id: &str) -> String {
        let known = self.names.try_borrow().ok().and_then(|names| {
            let found = names.iter().find(|(model, _)| model == id);
            found.map(|(_, name)| name.clone())
        });
        known.unwrap_or_else(|| {
            let name = model_name(self.thread, id);
            if let Ok(mut names) = self.names.try_borrow_mut() {
                names.push((id.to_owned(), name.clone()));
            }
            name
        })
    }

    /// Files a turn's steps changed, from its messages as read: each path an edit names, once.
    fn changed(&self, read: &[Option<Said>]) -> usize {
        let calls = read.iter().flatten().flat_map(|said| &said.blocks);
        let mut paths: Vec<String> = calls
            .filter_map(|block| match block {
                Block::Call(id) => self.transcript.tools.get(id.as_str()),
                Block::Text(_) | Block::Thinking(_) => None,
            })
            .filter(|call| call.details.get("added").is_some())
            .filter_map(tools::target)
            .collect();
        paths.sort();
        paths.dedup();
        paths.len()
    }
}

/// Where a message sits: its number in the transcript, its turn, and the files that turn's
/// steps changed.
#[derive(Clone, Copy)]
struct Place {
    at: usize,
    turn: Option<Turn>,
    files: usize,
}

/// Puts the tool rows gathered so far into `out` as one block with no gap between them.
fn flush(steps: &mut Vec<El>, out: &mut Vec<El>) {
    if !steps.is_empty() {
        out.push(col().w_full().children(std::mem::take(steps)));
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{clean, count, span};

    #[test]
    fn server_text_loses_direction_marks_and_stops_at_the_cap() -> Result<(), Box<dyn Error>> {
        let (shown, total) = clean("ab\u{202e}cd\u{7}e", 3);
        assert_eq!((shown.as_str(), total), ("abc", 5));
        let (whole, n) = clean("été", 3);
        assert_eq!(
            (whole.as_str(), n),
            ("été", 3),
            "a cap counts characters, not bytes"
        );
        Ok(())
    }

    #[test]
    fn counts_and_spans_read_as_the_record_writes_them() -> Result<(), Box<dyn Error>> {
        assert_eq!(count(41_200), "41.2K");
        assert_eq!(count(612), "612");
        assert_eq!(span(3_400), "3.4 s");
        assert_eq!(span(125_000), "2m 5s");
        Ok(())
    }
}
