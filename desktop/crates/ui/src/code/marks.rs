//! Line tints, gutter marks, squiggles, hints and followable names: what a caller puts on
//! lines of code, as data. A mark for a line or bytes the text does not have draws nothing.

use std::collections::BTreeMap;

use yd_core::row::Tone;

use super::doc::{Cols, LineNo};

/// A line's background and the 2 px bar at its left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tint {
    /// Added in this thread: a green bar and a `+` sign, no fill (record 4.3).
    Added,
    /// Removed: a red fill, a bar and a `-` sign.
    Removed,
    /// Where the debugger stopped: a fill and a bar (record 6.2).
    Current,
}

/// A mark in the gutter's first column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GutterMark {
    Breakpoint,
}

/// A small glyph at the very start of a line, over the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// A fix is on offer for this line.
    Fix,
}

/// Everything drawn on one line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LineMarks {
    pub(crate) tint: Option<Tint>,
    pub(crate) gutter: Option<GutterMark>,
    pub(crate) glyph: Option<Glyph>,
    /// Wavy lines under source bytes, each in the colour of its tone.
    pub(crate) squiggles: Vec<(Cols, Tone)>,
    /// Labels put in before a source byte, in byte order.
    pub(crate) hints: Vec<(u32, String)>,
    /// A label after the line's end.
    pub(crate) value: Option<String>,
    /// Names that can be followed; `true` for the one whose references are open.
    pub(crate) follows: Vec<(Cols, bool)>,
}

/// The marks of a text, by line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    lines: BTreeMap<LineNo, LineMarks>,
    /// The tint of every line that has none of its own: a file that is new in this thread.
    every: Option<Tint>,
}

impl Marks {
    pub fn new() -> Marks {
        Marks::default()
    }

    /// The marks of `line`, if it has any.
    pub fn line(&self, line: LineNo) -> Option<&LineMarks> {
        self.lines.get(&line)
    }

    pub fn tint(mut self, line: LineNo, tint: Tint) -> Marks {
        self.lines.entry(line).or_default().tint = Some(tint);
        self
    }

    /// Tints every line that has no tint of its own: a whole file added in this thread.
    pub fn tint_all(mut self, tint: Tint) -> Marks {
        self.every = Some(tint);
        self
    }

    /// The tint `line` is drawn with: its own, else the one every line has.
    pub(crate) fn tint_of(&self, line: LineNo) -> Option<Tint> {
        self.line(line).and_then(|marks| marks.tint).or(self.every)
    }

    pub fn gutter(mut self, line: LineNo, mark: GutterMark) -> Marks {
        self.lines.entry(line).or_default().gutter = Some(mark);
        self
    }

    pub fn glyph(mut self, line: LineNo, glyph: Glyph) -> Marks {
        self.lines.entry(line).or_default().glyph = Some(glyph);
        self
    }

    /// A wavy line under source bytes `cols` of `line` in the colour of `tone`: `Need` for a
    /// warning, `Bad` for an error.
    pub fn squiggle(mut self, line: LineNo, cols: Cols, tone: Tone) -> Marks {
        self.lines
            .entry(line)
            .or_default()
            .squiggles
            .push((cols, tone));
        self
    }

    /// A small label put in before source byte `before` of `line` (a type hint).
    pub fn hint(mut self, line: LineNo, before: u32, label: impl Into<String>) -> Marks {
        let hints = &mut self.lines.entry(line).or_default().hints;
        let at = hints.partition_point(|(col, _)| *col <= before);
        hints.insert(at, (before, label.into()));
        self
    }

    /// A label after the end of `line` (a value while debugging).
    pub fn value(mut self, line: LineNo, label: impl Into<String>) -> Marks {
        self.lines.entry(line).or_default().value = Some(label.into());
        self
    }

    /// A name that can be followed: source bytes `cols` of `line`, `open` while its references show.
    pub fn follow(mut self, line: LineNo, cols: Cols, open: bool) -> Marks {
        self.lines
            .entry(line)
            .or_default()
            .follows
            .push((cols, open));
        self
    }

    /// The most cells any line's hints and end label add to it: room to scroll sideways.
    pub(crate) fn widest(&self) -> usize {
        let added = |line: &LineMarks| {
            let hints = line
                .hints
                .iter()
                .map(|(_, label)| label.len().saturating_add(2));
            let value = line
                .value
                .as_ref()
                .map_or(0, |label| label.len().saturating_add(6));
            hints.fold(value, usize::saturating_add)
        };
        self.lines.values().map(added).max().unwrap_or(0)
    }
}
