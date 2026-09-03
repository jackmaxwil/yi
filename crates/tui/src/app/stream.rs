//! U13's streaming commit path: prose and reasoning both reach scrollback a stable slice at a
//! time, so the live region holds only the unstable tail and outgrowing it loses nothing.

use std::cmp::Ordering;

use ratatui::text::Line;

use super::App;
use crate::cell::{Cell, TranscriptMode};

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
        let stream = crate::markdown::stable_stream(&self.live_thought);
        let fenced = stream.reopen.is_some();
        let stable = if fenced { 0 } else { stream.cut };
        let mut cut = stable.max(self.live_thought_cut);
        let (width, theme) = (self.content_width(), self.theme);
        let forced = overflow_cut(
            self.live_thought.get(cut..).unwrap_or_default(),
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
        self.commit_thought_to(self.live_thought.len());
    }

    /// U13: each newly stable slice renders standalone against a byte cursor. Re-rendering
    /// the whole prefix let trailing-blank trimming duplicate list items mid-stream.
    pub(super) fn commit_stable_prefix(&mut self) {
        let stream = crate::markdown::stable_stream(&self.live_markdown);
        if stream.cut > self.live_cut {
            self.commit_prose(stream.cut, true);
            self.live_reopen = stream.reopen;
        }
        let (width, theme) = (self.content_width(), self.theme);
        let inner = width.saturating_sub(crate::cell::GUTTER.len());
        if let Some(forced) = overflow_cut(
            self.live_markdown.get(self.live_cut..).unwrap_or_default(),
            crate::render::live_tail_rows(self.rows),
            width,
            |text| crate::markdown::render(text, inner, &theme).len(),
        ) {
            self.commit_prose(self.live_cut + forced, false);
        }
    }

    /// `spaced` is false for a forced cut: inside a paragraph a blank line reads as a break
    /// the text does not have — unless it opens the block, which is a break and needs air.
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
        let first = self.live_cut == 0;
        let (rendered, lang) = crate::transcript::paint_slice(self, &slice);
        self.live_lang = lang;
        if !rendered.is_empty() {
            if (spaced || first) && self.live_reopen.is_none() {
                self.pending_commit.push(Line::default());
            }
            self.pending_commit
                .extend(crate::cell::gutter(rendered, first, &self.theme));
            self.retain(Cell::Assistant { markdown: slice });
        }
        self.live_cut = cut;
    }
}
