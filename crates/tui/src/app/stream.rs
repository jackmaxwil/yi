//! U13's streaming commit path: prose and reasoning both reach scrollback a stable slice at a
//! time, so the live region holds only the unstable tail and outgrowing it loses nothing.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::text::Line;
use serde_json::Value;
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult, apply};
use yi_types::message::{AgentMessage, Content};

use super::{App, TaskState, UiEvent};
use crate::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use crate::motion::elapsed_ms;
use crate::reveal::{FRAME, Reveal};
use crate::transcript::{preview_lines, text_of, todo_finished};

/// Both reveal cursors, their speed, and the events waiting behind them.
pub(crate) struct Pacing {
    pub(crate) prose: Reveal,
    pub(crate) thought: Reveal,
    pace: u16,
    pub(crate) held: VecDeque<UiEvent>,
}

impl Pacing {
    pub(crate) fn new(pace: u16) -> Self {
        Self {
            prose: Reveal::default(),
            thought: Reveal::default(),
            pace,
            held: VecDeque::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.prose.reset();
        self.thought.reset();
    }

    pub(crate) fn drain(&mut self) {
        self.prose.drain();
        self.thought.drain();
    }
}

/// The byte of `tail` to cut at so the rest renders within `budget` rows, or `None` when it
/// fits. No `stable_cut` boundary is guaranteed: one paragraph can outgrow the screen.
fn overflow_cut(
    tail: &str,
    budget: usize,
    width: usize,
    rows: impl Fn(&str) -> usize,
) -> Option<usize> {
    // Two scans before the render, the expensive half, which runs on every delta rather than
    // once a frame: text filling neither the budget's rows nor its columns cannot overrun.
    if tail.lines().count().max(tail.len() / width.max(1)) <= budget {
        return None;
    }
    // An open fence has no word boundary worth cutting on — broken open it
    // renders as an unterminated code block and its tail as prose.
    if tail.matches("```").count() % 2 == 1 || rows(tail) <= budget {
        return None;
    }
    let breaks: Vec<usize> = tail
        .match_indices(char::is_whitespace)
        .map(|(at, ws)| at + ws.len())
        .collect();
    // Monotone in the cut, so the first break that fits is the least the reader
    // loses from the live region. Never `Equal`, so the search never succeeds.
    let index = breaks
        .binary_search_by(|&at| {
            if rows(tail.get(at..).unwrap_or_default()) <= budget {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        })
        .unwrap_or_else(|index| index);
    // Cutting on that arbitrary word leaves the head on a half-empty row every screenful.
    // The last word that fits the row the break lands on wraps the seam like any other.
    let rest = breaks.get(index..)?;
    let head = rows(tail.get(..*rest.first()?).unwrap_or_default());
    let fills = rest.partition_point(|&at| rows(tail.get(..at).unwrap_or_default()) <= head);
    rest.get(fills.saturating_sub(1)).copied()
}

impl App {
    /// Invariant: thought commits on its own stable cuts and whole before any prose commits
    /// (`commit_prose` flushes it), so reasoning never lands under its answer.
    pub(super) fn commit_stable_thought(&mut self) {
        // `normal` renders a whole thought as one line of count; slicing it
        // would print that line once per paragraph.
        if self.mode == TranscriptMode::Normal {
            return;
        }
        // A cut inside a fence is prose's business: thought has no reopen to
        // carry, so a fenced block commits whole or not at all.
        let shown = self.pacing.thought.shown();
        let stream =
            crate::markdown::stable_stream(self.live_thought.get(..shown).unwrap_or_default());
        let fenced = stream.reopen.is_some();
        let stable = if fenced { 0 } else { stream.cut };
        let mut cut = stable.max(self.live_thought_cut);
        let (width, theme) = (self.content_width(), self.theme);
        let forced = overflow_cut(
            self.live_thought.get(cut..shown).unwrap_or_default(),
            crate::render::live_tail_rows(self.rows),
            width,
            |text| {
                crate::cell::thought_lines(text, width, &theme, TranscriptMode::Thinking, false)
                    .len()
            },
        );
        cut += forced.unwrap_or(0);
        if cut > self.live_thought_cut {
            self.commit_thought_to(cut);
        }
    }

    /// Reads `live_thought_cut` for the label, so the cut moves last.
    fn commit_thought_to(&mut self, cut: usize) {
        let slice = self
            .live_thought
            .get(self.live_thought_cut..cut)
            .unwrap_or_default()
            .to_owned();
        if !slice.trim().is_empty() {
            let lines = crate::cell::thought_lines(
                &slice,
                self.content_width(),
                &self.theme,
                self.mode,
                self.live_thought_cut == 0,
            );
            self.last_commit_rows = lines.len();
            self.pending_commit.extend(lines);
            self.retain(Cell::Thought { markdown: slice });
            self.scheduler.request();
        }
        self.live_thought_cut = cut;
    }

    pub(super) fn flush_thought(&mut self) {
        self.pacing.thought.snap(self.live_thought.len());
        self.commit_thought_to(self.live_thought.len());
    }

    /// U13: each newly stable slice renders standalone against a byte cursor. Re-rendering
    /// the whole prefix let trailing-blank trimming duplicate list items mid-stream.
    pub(super) fn commit_stable_prefix(&mut self) {
        let shown = self.pacing.prose.shown();
        let stream =
            crate::markdown::stable_stream(self.live_markdown.get(..shown).unwrap_or_default());
        if stream.cut > self.live_cut {
            self.commit_prose(stream.cut, true);
            self.live_reopen = stream.reopen;
        }
        let (width, theme) = (self.content_width(), self.theme);
        let inner = width.saturating_sub(crate::cell::GUTTER.len());
        if let Some(forced) = overflow_cut(
            self.live_markdown
                .get(self.live_cut..shown)
                .unwrap_or_default(),
            crate::render::live_tail_rows(self.rows),
            width,
            |text| crate::markdown::render(text, inner, &theme).len(),
        ) {
            self.commit_prose(self.live_cut + forced, false);
        }
    }

    /// `spaced` is false for a forced cut: the block after it continues a paragraph, so it
    /// gets no blank line above it; a stable cut ends one, so the next block needs air.
    pub(super) fn commit_prose(&mut self, cut: usize, spaced: bool) {
        if cut <= self.live_cut {
            return;
        }
        self.flush_thought();
        let slice = self
            .live_markdown
            .get(self.live_cut..cut)
            .unwrap_or_default()
            .to_owned();
        let (lines, lang) = self.prose_block(&slice);
        self.live_lang = lang;
        if !lines.is_empty() {
            self.pending_commit.extend(lines);
            self.retain(Cell::Assistant { markdown: slice });
        }
        self.live_cut = cut;
        self.live_spaced = spaced;
    }

    /// Incident: live and commit each rendered the slice and only the commit put a blank
    /// line above it, so every stable cut pushed text the reader had seen down a row.
    pub(crate) fn prose_block(
        &self,
        slice: &str,
    ) -> (Vec<Line<'static>>, Option<crate::highlight::Lang>) {
        let (rendered, lang) = crate::transcript::paint_slice(self, slice);
        let mut lines = Vec::with_capacity(rendered.len().saturating_add(1));
        if !rendered.is_empty() && self.live_spaced && self.live_reopen.is_none() {
            lines.push(Line::default());
        }
        lines.extend(crate::cell::gutter(
            rendered,
            self.live_cut == 0,
            &self.theme,
        ));
        (lines, lang)
    }

    /// D145: a whole message (`Start`, `Done`, `Error`) opens or settles the stream;
    /// a delta grows the message `MessageStart` opened.
    pub(super) fn fold_stream(&mut self, event: &AssistantMessageEvent) {
        fold(&mut self.streaming, event);
        if let Some(AgentMessage::Assistant { content, .. }) = &self.streaming {
            let content = content.clone();
            self.arrive(&content);
        }
    }

    /// A snapshot of the message so far: the arrival feeds the rate, the cursors move on
    /// their own clock, and nothing commits before the reader has seen it.
    pub(super) fn arrive(&mut self, content: &[Content]) {
        let now = Instant::now();
        self.live_markdown = crate::transcript::text_of(content);
        self.live_thought = crate::transcript::thinking_of(content);
        self.pacing.prose.on_arrival(self.live_markdown.len(), now);
        self.pacing.thought.on_arrival(self.live_thought.len(), now);
        self.step_reveal(now);
    }

    /// Moves both cursors by the time since the last tick; true when a frame is owed.
    pub fn step_reveal(&mut self, now: Instant) -> bool {
        let pace = self.pacing.pace;
        let moved = self.pacing.thought.advance(&self.live_thought, now, pace)
            | self.pacing.prose.advance(&self.live_markdown, now, pace);
        if moved {
            self.commit_stable_thought();
            self.commit_stable_prefix();
            self.scheduler.request();
        }
        moved
    }

    pub(crate) fn reveal_behind(&self) -> bool {
        self.pacing.prose.behind(self.live_markdown.len())
            || self.pacing.thought.behind(self.live_thought.len())
    }

    /// The next wake while text is still unrevealed, `Duration::MAX` once it is all shown.
    pub(crate) fn reveal_wake(&self) -> Duration {
        if self.reveal_behind() {
            FRAME
        } else {
            Duration::MAX
        }
    }
}

fn fold(streaming: &mut Option<AgentMessage>, event: &AssistantMessageEvent) {
    match event {
        AssistantMessageEvent::Start { partial: whole }
        | AssistantMessageEvent::Done { message: whole, .. }
        | AssistantMessageEvent::Error { error: whole, .. } => *streaming = Some(whole.clone()),
        delta => {
            if let Some(message) = streaming.as_mut() {
                apply(message, delta);
            }
        }
    }
}

/// A child's stream folds into its cell's answer; true when the cell changed.
pub(super) fn fold_child(
    tasks: &mut std::collections::HashMap<String, TaskState>,
    child_id: &str,
    event: &AgentEvent,
) -> bool {
    let Some(state) = tasks.get_mut(child_id) else {
        return false;
    };
    match event {
        AgentEvent::MessageStart {
            message: message @ AgentMessage::Assistant { .. },
        } => {
            state.streaming = Some(message.clone());
            false
        }
        AgentEvent::MessageUpdate {
            assistant_message_event,
        } => {
            fold(&mut state.streaming, assistant_message_event);
            if let Some(AgentMessage::Assistant { content, .. }) = &state.streaming {
                state.cell.answer = Some(crate::cell::tail_bounded(crate::transcript::text_of(
                    content,
                )));
                return true;
            }
            false
        }
        _ => false,
    }
}

impl App {
    /// A finished call leaves the live region for scrollback as its card; a todo step leaves
    /// nothing, since the HUD carries the list, and only the list's close is a row.
    pub(super) fn tool_ended(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        result: &ToolResult,
        is_error: bool,
    ) {
        self.turn_tools = self.turn_tools.saturating_add(1);
        let elapsed = self
            .tool_started
            .remove(&tool_call_id)
            .map(elapsed_ms)
            .unwrap_or(0);
        let index = self
            .live_tools
            .iter()
            .position(|tool| tool.call_id == tool_call_id)
            .or_else(|| {
                self.live_tools
                    .iter()
                    .position(|tool| tool.status != ToolStatus::Done && tool.name == tool_name)
            });
        let mut cell = match index {
            Some(i) => self.live_tools.remove(i),
            // No start was seen, so nothing is known but the name; the
            // result below fills in the rest.
            None => ToolCell {
                name: tool_name.clone(),
                call_id: tool_call_id.clone(),
                intent: None,
                status: ToolStatus::Running,
                summary: ToolCell::summary_of(&tool_name, ""),
                digest: None,
                preview: Vec::new(),
                elapsed_ms: 0,
                calls: 1,
                details: Value::Null,
            },
        };
        cell.status = if is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        };
        cell.elapsed_ms = elapsed;
        let text = text_of(&result.content);
        cell.digest = ToolCell::digest_of(&tool_name, &text, is_error);
        cell.preview = preview_lines(&text, 12, 6);
        cell.details = result.details.clone();
        // The HUD carries the list; a card per step was six cards a turn.
        if tool_name == "todo" && !is_error {
            if let Some(done) = todo_finished(&text) {
                self.commit_cell(&Cell::Footer { text: done });
            }
        } else {
            self.commit_cell(&Cell::Tool(cell));
        }
        self.commit_finished_tasks();
        self.intent = None;
    }
}
