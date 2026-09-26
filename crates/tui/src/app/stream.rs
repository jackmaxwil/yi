//! §17.3's streaming commit path: prose and reasoning both reach scrollback a stable slice at a
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

#[derive(Default, Clone)]
pub(crate) struct Seam {
    pub(crate) stub: Option<String>,
    pub(crate) mid: bool,
}

impl Seam {
    fn after(cut: &Cut) -> Self {
        Self {
            stub: cut.stub.clone(),
            mid: cut.word && !cut.line,
        }
    }
}

/// Incident: rendering a cut block's whole head cost 1.3 ms a frame at 20 KB, so a slice renders
/// under one line, [`Cut::stub`]: the item before the cut, or the item or quote it falls in.
#[derive(Clone)]
struct Cut {
    at: usize,
    spaced: bool,
    stub: Option<String>,
    word: bool,
    /// At the start of a line, where the text left keeps its own structure.
    line: bool,
}

/// Where to cut `tail` (rendered under `context`) so the rest fits `budget` rows; `None` when it
/// fits or nothing can be cut. No boundary is guaranteed: a paragraph can outgrow the screen.
fn overflow_cut(
    context: &str,
    tail: &str,
    budget: usize,
    width: usize,
    rows: impl Fn(&str) -> usize,
) -> Option<Cut> {
    // Two scans before the render, the expensive half, which runs on every delta rather than
    // once a frame: text filling neither the budget's rows nor its columns cannot overrun.
    if tail.lines().count().max(tail.len() / width.max(1)) <= budget {
        return None;
    }
    // An open fence has no word boundary worth cutting on — broken open it
    // renders as an unterminated code block and its tail as prose.
    if crate::markdown::open_fence(tail) {
        return None;
    }
    let source = format!("{context}{tail}");
    let from = context.len();
    let breaks: Vec<Cut> = breaks(&source)
        .into_iter()
        .filter(|cut| cut.at > from)
        .collect();
    if breaks.is_empty() {
        return None;
    }
    let head = |at: usize| rows(source.get(..at).unwrap_or_default());
    let total = rows(&source);
    if total.saturating_sub(head(from)) <= budget {
        return None;
    }
    // Monotone in the cut, so the first break that fits is the least the reader
    // loses from the live region. Never `Equal`, so the search never succeeds.
    let index = breaks
        .binary_search_by(|cut| {
            if total.saturating_sub(head(cut.at)) <= budget {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        })
        .unwrap_or_else(|index| index);
    // Incident: `- top 0` cut after `top` dropped the `0`: the word after a cut must open a
    // row, or the rows before it differ from the whole's (an inline span wraps inside itself).
    let seam = |cut: &Cut| {
        !cut.word || {
            let rest = source.get(cut.at..).unwrap_or_default();
            let word = rest.trim_start().find(char::is_whitespace);
            let next = word.map_or(source.len(), |end| {
                cut.at + (rest.len() - rest.trim_start().len()) + end
            });
            head(cut.at) < head(next)
        }
    };
    let found = match breaks.get(index..).filter(|rest| !rest.is_empty()) {
        None => breaks.iter().rev().find(|cut| !cut.word).cloned(),
        // Cutting on that arbitrary word leaves the head on a half-empty row every screenful.
        // The last word that fits the row the break lands on wraps the seam like any other.
        Some(rest) => {
            let filled = head(rest.first()?.at);
            let fills = rest.partition_point(|cut| head(cut.at) <= filled);
            let (row, later) = rest.split_at(fills);
            row.iter().rev().chain(later).find(|cut| seam(cut)).cloned()
        }
    };
    found.map(|cut| Cut {
        at: cut.at - from,
        ..cut
    })
}

/// Incident: cuts inside `**…**`, after a bullet or in a table lost the markup. A forced cut
/// lands between words of a top-level paragraph, item or quote, or at a block or item start.
fn breaks(source: &str) -> Vec<Cut> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let mut cuts: Vec<Cut> = Vec::new();
    let mut prose: Vec<(std::ops::Range<usize>, Option<usize>, Option<String>)> = Vec::new();
    let mut atomic: Vec<std::ops::Range<usize>> = Vec::new();
    // The open top-level list: (start number, items seen).
    let mut list: (Option<u64>, u64) = (None, 0);
    let mut depth = 0usize;
    // A block's range can start past its indent, and a cut there reads `\t```` as a fence.
    let start = |at: usize, spaced: bool, stub: Option<String>| Cut {
        at: crate::markdown::line_start(source, at),
        spaced,
        stub,
        word: false,
        line: true,
    };
    let parser = Parser::new_ext(
        source,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    );
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                match tag {
                    Tag::Paragraph if depth == 0 => {
                        cuts.push(start(range.start, true, None));
                        prose.push((range, None, None));
                    }
                    Tag::BlockQuote(_) if depth == 0 => {
                        cuts.push(start(range.start, true, None));
                        let stub = format!(
                            "{}> x  \n",
                            crate::markdown::stub_prefix(source, range.start)
                        );
                        prose.push((range.clone(), Some(range.start), Some(stub)));
                    }
                    Tag::List(first) if depth == 0 => {
                        cuts.push(start(range.start, true, None));
                        list = (first, 0);
                    }
                    _ if depth == 0 => cuts.push(start(range.start, true, None)),
                    Tag::Item if depth == 1 => {
                        let number = list.0.map(|first| first.saturating_add(list.1));
                        let before = (list.1 > 0).then(|| item_stub(source, range.start, list));
                        list.1 += 1;
                        if before.is_some() {
                            cuts.push(start(range.start, false, before));
                        }
                        // A word cut inside it renders under its marker line ending in a hard break.
                        let own = crate::markdown::item_line(source, range.start, number, "  \n");
                        prose.push((range.clone(), Some(range.start), Some(own)));
                    }
                    Tag::Paragraph => {}
                    _ => atomic.push(range),
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Code(_) | Event::InlineHtml(_) => atomic.push(range),
            _ => {}
        }
    }
    let inside = |at: usize, range: &std::ops::Range<usize>| range.start < at && at < range.end;
    for (at, ws) in source.match_indices(char::is_whitespace) {
        let at = at + ws.len();
        let Some((_, block, stub)) = prose.iter().find(|(range, _, _)| inside(at, range)) else {
            continue;
        };
        // An item's head must hold a word past its marker, or it renders as the bare marker.
        let worded = block.is_none_or(|from| {
            source
                .get(from..at)
                .is_some_and(|head| head.split_whitespace().nth(1).is_some())
        });
        // After a line's indent the text left would lose the indent a nested block needs.
        let indented = source
            .get(crate::markdown::line_start(source, at)..at)
            .is_some_and(|lead| !lead.is_empty() && lead.trim().is_empty());
        if worded && !indented && !atomic.iter().any(|range| inside(at, range)) {
            let line = source.get(..at).is_some_and(|head| head.ends_with('\n'));
            // After a blank line the text left opens a paragraph, not a hard-broken line.
            let stub =
                stub.clone().map(
                    |stub| match line && crate::markdown::blank_before(source, at) {
                        true => format!("{}\n\n", stub.trim_end()),
                        false => stub,
                    },
                );
            cuts.push(Cut {
                at,
                spaced: false,
                stub,
                word: true,
                line,
            });
        }
    }
    // A row still arriving (`| ` alone) parses outside the table; its lines decide where it ends.
    let table = table_span(source);
    cuts.retain(|cut| {
        cut.at > 0
            && !table.as_ref().is_some_and(|table| inside(cut.at, table))
            && !crate::markdown::undecided(source, cut.at)
    });
    cuts.sort_by_key(|cut| cut.at);
    cuts.dedup_by_key(|cut| cut.at);
    cuts
}

/// The item before the one at `at`, as one line numbered as the list shows it.
fn item_stub(source: &str, at: usize, (first, seen): (Option<u64>, u64)) -> String {
    let indent = crate::markdown::stub_prefix(source, at);
    let marker = crate::markdown::item_marker(source.get(at..).unwrap_or_default()).unwrap_or("-");
    let marker = match first {
        Some(first) => format!(
            "{}{}",
            first.saturating_add(seen).saturating_sub(1),
            marker.chars().last().unwrap_or('.')
        ),
        None => marker.to_owned(),
    };
    let gap = if crate::markdown::blank_before(source, at) {
        "\n"
    } else {
        ""
    };
    format!("{indent}{marker} x\n{gap}")
}

/// The tail's context (a stand-in prefix, then the head of the block a cut fell inside) and the
/// The tail's context: the seam's stub, then the escape a mid-paragraph `- ` wears.
fn context(seam: &Seam, tail: &str) -> String {
    let escape = seam.mid && crate::transcript::escaped(tail, true) != tail;
    format!(
        "{}{}",
        seam.stub.as_deref().unwrap_or_default(),
        if escape { "\\" } else { "" }
    )
}

pub(crate) fn table_span(tail: &str) -> Option<std::ops::Range<usize>> {
    let (mut start, mut offset, mut previous) = (None, 0, 0);
    for line in tail.split_inclusive('\n') {
        let row = line.trim();
        let rule = row.contains('|')
            && row.contains('-')
            && row.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
        let ends = row.is_empty()
            || ["- ", "* ", "+ ", "#", ">", "```", "~~~"]
                .iter()
                .any(|mark| row.starts_with(mark));
        match start {
            None if rule => start = Some(previous),
            None if row.starts_with('|') => start = Some(offset),
            Some(from) if ends => return Some(from..offset),
            _ => {}
        }
        previous = offset;
        offset += line.len();
    }
    start.map(|from| from..tail.len())
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
        let (shown, base) = (self.pacing.thought.shown(), self.live_thought_base);
        let stream =
            crate::markdown::stable_stream(self.live_thought.get(base..shown).unwrap_or_default());
        if stream.reopen.is_none() && base + stream.cut > self.live_thought_cut {
            self.commit_thought_to(base + stream.cut, true);
            self.live_thought_seam = Seam::default();
            self.live_thought_base = base + stream.block;
        }
        let (width, theme) = (self.content_width(), self.theme);
        let tail = self
            .live_thought
            .get(self.live_thought_cut..shown)
            .unwrap_or_default();
        let context = context(&self.live_thought_seam, tail);
        let cut = overflow_cut(
            &context,
            tail,
            crate::render::live_tail_rows(self.rows),
            width,
            |text| {
                crate::cell::thought_lines(text, width, &theme, TranscriptMode::Thinking, false)
                    .len()
            },
        );
        if let Some(cut) = cut {
            self.commit_thought_to(self.live_thought_cut + cut.at, cut.spaced);
            self.live_thought_seam = Seam::after(&cut);
        }
    }

    /// Reads `live_thought_cut` for the label, so the cut moves last.
    fn commit_thought_to(&mut self, cut: usize, spaced: bool) {
        if cut <= self.live_thought_cut {
            return;
        }
        let slice = self
            .live_thought
            .get(self.live_thought_cut..cut)
            .unwrap_or_default()
            .to_owned();
        if !slice.trim().is_empty() {
            // Invariant: a held run of reads commits above the reasoning that follows it.
            self.flush_explored();
            let lines = self.thought_block(&slice);
            self.note_commit(&lines, self.live_thought_cut > 0);
            let rows = (self.live_thought_cut > 0).then(|| (self.content_width(), lines.clone()));
            self.history.retain_slice(
                Cell::Thought { markdown: slice },
                rows.as_ref().map(|(width, rows)| (*width, rows.as_slice())),
            );
            self.pending_commit.extend(lines);
            self.scheduler.request();
        } else {
            self.history.retain(Cell::Thought { markdown: slice });
        }
        self.live_thought_cut = cut;
        self.live_thought_spaced = spaced;
    }

    pub(crate) fn thought_block(&self, slice: &str) -> Vec<Line<'static>> {
        let (width, theme, mode) = (self.content_width(), &self.theme, self.mode);
        let seam = &self.live_thought_seam;
        let slice = crate::transcript::escaped(slice, seam.mid);
        let render = |text: &str| crate::cell::thought_lines(text, width, theme, mode, false);
        if let Some(rows) = seam
            .stub
            .as_deref()
            .and_then(|stub| crate::transcript::under(stub, &slice, render))
        {
            return rows;
        }
        let header = self.live_thought_cut == 0 || self.live_thought_spaced;
        crate::cell::thought_lines(&slice, width, theme, mode, header)
    }

    /// A message's slices are one history cell, so the separator rule measures their sum.
    fn note_commit(&mut self, lines: &[Line<'static>], continues: bool) {
        let Some(last) = lines.last() else {
            return;
        };
        self.last_commit_rows = if continues {
            self.last_commit_rows.saturating_add(lines.len())
        } else {
            lines.len()
        };
        self.last_commit_blank = crate::history::is_blank(last);
    }

    pub(super) fn flush_thought(&mut self) {
        self.pacing.thought.snap(self.live_thought.len());
        self.commit_thought_to(self.live_thought.len(), true);
    }

    /// Each newly stable slice renders standalone against a byte cursor. Re-rendering
    /// the whole prefix let trailing-blank trimming duplicate list items mid-stream.
    pub(super) fn commit_stable_prefix(&mut self) {
        let (shown, base) = (self.pacing.prose.shown(), self.live_base);
        let stream =
            crate::markdown::stable_stream(self.live_markdown.get(base..shown).unwrap_or_default());
        if base + stream.cut > self.live_cut {
            self.commit_prose(base + stream.cut, true);
            self.live_reopen = stream.reopen;
            self.live_seam = Seam::default();
            self.live_base = base + stream.block;
        }
        if self.live_reopen.is_some() {
            return;
        }
        let (width, theme) = (self.content_width(), self.theme);
        let inner = width.saturating_sub(crate::cell::gutter_cols());
        let tail = self
            .live_markdown
            .get(self.live_cut..shown)
            .unwrap_or_default();
        let context = context(&self.live_seam, tail);
        let cut = overflow_cut(
            &context,
            tail,
            crate::render::live_tail_rows(self.rows),
            width,
            |text| crate::markdown::render(text, inner, &theme).len(),
        );
        if let Some(cut) = cut {
            self.commit_prose(self.live_cut + cut.at, cut.spaced);
            self.live_seam = Seam::after(&cut);
        }
    }

    /// `spaced` is false for a forced cut: the block after it continues a paragraph, so it
    /// gets no blank line above it; a stable cut ends one, so the next block needs air.
    pub(super) fn commit_prose(&mut self, cut: usize, spaced: bool) {
        if cut <= self.live_cut {
            return;
        }
        self.flush_thought();
        self.flush_explored();
        let slice = self
            .live_markdown
            .get(self.live_cut..cut)
            .unwrap_or_default()
            .to_owned();
        let (lines, lang) = self.prose_block(&slice);
        self.live_lang = lang;
        let continues = self.live_drawn;
        self.note_commit(&lines, continues);
        // Incident: the slice holding only a fence's close renders no row, and dropping it
        // left the history's copy of the message inside the fence.
        let width = self.content_width();
        let rows = continues.then_some((width, lines.as_slice()));
        self.history
            .retain_slice(Cell::Assistant { markdown: slice }, rows);
        self.live_drawn |= !lines.is_empty();
        self.pending_commit.extend(lines);
        self.live_cut = cut;
        self.live_spaced = spaced;
        self.scheduler.request();
    }

    /// Incident: live and commit each rendered the slice and only the commit put a blank
    /// line above it, so every stable cut pushed text the reader had seen down a row.
    pub(crate) fn prose_block(
        &self,
        slice: &str,
    ) -> (Vec<Line<'static>>, Option<crate::highlight::Lang>) {
        let (rendered, lang) = crate::transcript::paint_slice(self, slice);
        let mut lines = Vec::with_capacity(rendered.len().saturating_add(1));
        // The first slice to draw opens on its history cell's blank, unless the row above is one.
        let opens = !self.live_drawn && !self.last_commit_blank;
        let spaced = self.live_drawn && self.live_spaced && self.live_reopen.is_none();
        if !rendered.is_empty() && (opens || spaced) {
            lines.push(Line::default());
        }
        lines.extend(crate::cell::gutter(rendered, !self.live_drawn, &self.theme));
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
        self.close_segments(content);
        let open = content.get(self.segment..).unwrap_or_default();
        self.live_markdown = crate::transcript::prose_of(open);
        self.live_thought = crate::transcript::thinking_of(open);
        self.pacing.prose.on_arrival(self.live_markdown.len(), now);
        self.pacing.thought.on_arrival(self.live_thought.len(), now);
        self.step_reveal(now);
    }

    /// Incident: a thought after prose drew above prose the reader had already seen; what came
    /// before the thought commits whole above it.
    pub(crate) fn close_segments(&mut self, content: &[Content]) {
        loop {
            let open = content.get(self.segment..).unwrap_or_default();
            let mut spoke = false;
            let Some(split) = open.iter().position(|block| {
                let thought = matches!(block, Content::Thinking { .. });
                let beat = spoke && thought;
                spoke |= matches!(block, Content::Text { text, .. } if !text.is_empty());
                beat
            }) else {
                return;
            };
            let head = open.get(..split).unwrap_or_default();
            self.live_thought = crate::transcript::thinking_of(head);
            self.flush_thought();
            self.live_markdown = crate::transcript::prose_of(head);
            self.pacing.prose.snap(self.live_markdown.len());
            self.commit_prose(self.live_markdown.len(), true);
            self.clear_live();
            self.history.seal();
            self.segment += split;
        }
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

    pub(crate) fn reveal_wake(&self) -> Duration {
        let moving = |reveal: &Reveal, len: usize| reveal.behind(len) && !reveal.waiting();
        if moving(&self.pacing.prose, self.live_markdown.len())
            || moving(&self.pacing.thought, self.live_thought.len())
        {
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
