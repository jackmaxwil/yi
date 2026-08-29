//! U13's streaming commit path: the prose and the reasoning of a turn both
//! reach scrollback a stable slice at a time, so the live region only ever
//! holds the unstable tail and nothing is lost when that tail is outgrown.

use std::cmp::Ordering;

use ratatui::text::Line;

use super::App;
use crate::cell::{Cell, TranscriptMode};

/// The byte of `tail` to cut at so what is left renders within `budget` rows,
/// or `None` when it already fits. No `stable_cut` boundary is guaranteed: a
/// model can stream one paragraph longer than the screen, and the tail drops it.
fn overflow_cut(
    tail: &str,
    budget: usize,
    width: usize,
    rows: impl Fn(&str) -> usize,
) -> Option<usize> {
    // Two O(1) refusals before the render, because this runs on every delta
    // while the live region renders once a frame: text that can fill neither
    // the budget's rows nor its columns cannot overrun it.
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
    // Cutting on that arbitrary word leaves the head on a half-empty row, a
    // short line every screenful. The last word that still fits the row that
    // break lands on wraps the seam like any other line.
    let rest = breaks.get(index..)?;
    let head = rows(tail.get(..*rest.first()?).unwrap_or_default());
    let fills = rest.partition_point(|&at| rows(tail.get(..at).unwrap_or_default()) <= head);
    rest.get(fills.saturating_sub(1)).copied()
}

impl App {
    /// Thought commits on prose's stable-cut boundaries. The live region keeps
    /// only a half-screen tail, so a slice that never commits is gone for good —
    /// and committing here is what puts reasoning above the prose it preceded.
    pub(super) fn commit_stable_thought(&mut self) {
        // `normal` renders a whole thought as one line of count; slicing it
        // would print that line once per paragraph.
        if self.mode == TranscriptMode::Normal {
            return;
        }
        let mut cut = crate::markdown::stable_cut(&self.live_thought).max(self.live_thought_cut);
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
        if cut <= self.live_thought_cut {
            return;
        }
        let slice = self
            .live_thought
            .get(self.live_thought_cut..cut)
            .unwrap_or_default()
            .to_owned();
        self.commit_thought_slice(&slice);
        self.live_thought_cut = cut;
    }

    /// Reads `live_thought_cut` for the label, so it runs before the cut moves.
    pub(super) fn commit_thought_slice(&mut self, slice: &str) {
        if slice.trim().is_empty() {
            return;
        }
        let lines = crate::cell::thought_lines(
            slice,
            self.content_width(),
            &self.theme,
            self.mode,
            self.live_thought_cut == 0,
        );
        self.last_commit_rows = lines.len();
        self.pending_commit.extend(lines);
        self.retain(Cell::Thought {
            markdown: slice.to_owned(),
        });
        self.scheduler.request();
    }

    /// U13: each newly stable slice renders standalone against a byte cursor.
    /// Re-rendering the whole prefix let the renderer's trailing-blank trimming
    /// misalign the committed count and duplicate list items mid-stream.
    pub(super) fn commit_stable_prefix(&mut self) {
        self.commit_prose(crate::markdown::stable_cut(&self.live_markdown), true);
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

    /// `spaced` is false for a forced cut: it lands inside a paragraph, where a
    /// blank line would read as the break the text does not have.
    fn commit_prose(&mut self, cut: usize, spaced: bool) {
        if cut <= self.live_cut {
            return;
        }
        let slice = self
            .live_markdown
            .get(self.live_cut..cut)
            .unwrap_or_default()
            .to_owned();
        let first = self.live_cut == 0;
        let rendered = crate::markdown::render(
            &slice,
            self.content_width()
                .saturating_sub(crate::cell::GUTTER.len()),
            &self.theme,
        );
        if !rendered.is_empty() {
            if spaced {
                self.pending_commit.push(Line::default());
            }
            self.pending_commit
                .extend(crate::cell::gutter(rendered, first, &self.theme));
            self.retain(Cell::Assistant { markdown: slice });
        }
        self.live_cut = cut;
    }
}
