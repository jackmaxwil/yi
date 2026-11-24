//! One line: columns to points, the word under the pointer. The drawn text has hints and tab
//! stops put in; this maps each byte across the line and back to the source.

use std::ops::Range;

pub(crate) mod ink;

/// Why a stretch of the drawn text is not in the source. Neither kind is copied or hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// An inline hint: a label with a space each side, in the hint's smaller face.
    Hint,
    /// The spaces that carry a tab to its four-column stop. The tab itself is drawn as a space:
    /// a face decides how wide a tab is (the system mono face runs it to a stop of its own).
    TabStop,
}

/// The width of one character cell in the code's face and in the hint's, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Advance {
    pub code: f32,
    pub hint: f32,
}

/// A line's drawn text and the stretches of it that are not source.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    text: String,
    puts: Vec<(Range<usize>, Put)>,
    /// Every drawn character is one ASCII cell, so places are sums, not measurements.
    cells: bool,
}

impl Shown {
    /// `source` (one line, no line end) with each of `hints` put in before the source byte it
    /// names. Hints come in byte order; one inside a character or past the line is left out.
    pub fn new(source: &str, hints: &[(u32, impl AsRef<str>)]) -> Shown {
        let mut out = Shown {
            text: String::with_capacity(source.len()),
            puts: Vec::new(),
            cells: true,
        };
        let mut hints = hints.iter().peekable();
        let mut col = 0usize;
        for (at, ch) in source
            .char_indices()
            .map(|(i, c)| (i, Some(c)))
            .chain([(source.len(), None)])
        {
            while let Some((h, label)) =
                hints.next_if(|(h, _)| usize::try_from(*h).is_ok_and(|h| h <= at))
            {
                if usize::try_from(*h) != Ok(at) {
                    continue;
                }
                let start = out.text.len();
                out.text.extend([" ", label.as_ref(), " "]);
                out.cells &= label.as_ref().bytes().all(is_cell);
                out.puts.push((start..out.text.len(), Put::Hint));
            }
            let Some(ch) = ch else { break };
            out.text.push(if ch == '\t' { ' ' } else { ch });
            out.cells &= ch == '\t' || u8::try_from(ch).is_ok_and(is_cell);
            if ch == '\t' {
                let start = out.text.len();
                out.text
                    .extend(std::iter::repeat_n(' ', 3usize.saturating_sub(col % 4)));
                col = col.saturating_add(out.text.len().saturating_sub(start));
                out.puts.push((start..out.text.len(), Put::TabStop));
            }
            col = col.saturating_add(1);
        }
        out.puts.retain(|(r, _)| !r.is_empty());
        out
    }

    /// The text to draw.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The drawn stretches that are not source, in order.
    pub fn puts(&self) -> &[(Range<usize>, Put)] {
        &self.puts
    }

    /// Where source byte `col` is drawn: after anything put in before it.
    pub fn to_drawn(&self, col: usize) -> usize {
        let mut at = col;
        for (put, _) in &self.puts {
            if put.start > at {
                break;
            }
            at = at.saturating_add(put.len());
        }
        at
    }

    /// The drawn bytes of source bytes `cols`, which must hold whole characters: from where its
    /// first character is drawn to the end of its last, so nothing put in after it is taken.
    pub fn drawn(&self, source: &str, cols: Range<usize>) -> Option<Range<usize>> {
        let last = source.get(cols.clone())?.char_indices().next_back()?;
        let end = self
            .to_drawn(cols.start.saturating_add(last.0))
            .saturating_add(last.1.len_utf8());
        Some(self.to_drawn(cols.start)..end)
    }

    /// How many source bytes are drawn before byte `at`: where a selection that starts or ends
    /// there starts or ends in the source.
    pub fn source_before(&self, at: usize) -> usize {
        let put: usize = self
            .puts
            .iter()
            .map(|(r, _)| r.end.min(at).saturating_sub(r.start.min(at)))
            .sum();
        at.min(self.text.len()).saturating_sub(put)
    }

    /// The source byte drawn at byte `at`; nothing for a byte that was put in.
    pub fn to_source(&self, at: usize) -> Option<usize> {
        let mut added = 0usize;
        for (put, _) in &self.puts {
            if at < put.start {
                break;
            }
            if at < put.end {
                return None;
            }
            added = added.saturating_add(put.len());
        }
        Some(at.saturating_sub(added))
    }

    /// How far from the text's left edge drawn byte `at` starts. `measured` gives that for a
    /// line whose characters are not all one cell wide.
    pub fn x_of(&self, at: usize, adv: Advance, measured: impl Fn(usize) -> f32) -> f32 {
        if !self.cells {
            return measured(at.min(self.text.len()));
        }
        let hint: usize = self
            .hints()
            .map(|r| r.end.min(at).saturating_sub(r.start.min(at)))
            .sum();
        cells(at.min(self.text.len()).saturating_sub(hint)) * adv.code + cells(hint) * adv.hint
    }

    /// The drawn byte of the character under `x` points from the text's left edge; nothing
    /// before the text or past its end.
    pub fn at_x(&self, x: f32, adv: Advance, measured: impl Fn(usize) -> f32) -> Option<usize> {
        if x < 0.0 {
            return None;
        }
        if !self.cells {
            let starts: Vec<usize> = self
                .text
                .char_indices()
                .map(|(i, _)| i)
                .chain([self.text.len()])
                .collect();
            let after = starts.partition_point(|&at| measured(at) <= x);
            return starts
                .get(after.checked_sub(1)?)
                .copied()
                .filter(|&at| at < self.text.len());
        }
        let (mut left, mut from) = (0.0f32, 0usize);
        let end = self.text.len();
        for hint in self.hints().chain(std::iter::once(end..end)) {
            for (run, width) in [(from..hint.start, adv.code), (hint.clone(), adv.hint)] {
                let right = left + cells(run.len()) * width;
                if x < right && width > 0.0 {
                    let over = ((x - left) / width).floor();
                    return (0..run.len())
                        .rev()
                        .find(|&i| cells(i) <= over)
                        .map(|i| run.start.saturating_add(i));
                }
                left = right;
            }
            from = hint.end;
        }
        None
    }

    fn hints(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.puts
            .iter()
            .filter(|(_, put)| *put == Put::Hint)
            .map(|(r, _)| r.clone())
    }
}

/// The word of `source` (letters, digits and `_`) that holds byte `col`; nothing between words.
pub fn word_at(source: &str, col: usize) -> Option<Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let (before, from) = (source.get(..col)?, source.get(col..)?);
    from.chars().next().filter(|c| word(*c))?;
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| word(*c))
        .last()
        .map_or(col, |(i, _)| i);
    let len = from
        .char_indices()
        .find(|(_, c)| !word(*c))
        .map_or(from.len(), |(i, _)| i);
    Some(start..col.saturating_add(len))
}

fn is_cell(b: u8) -> bool {
    b.is_ascii_graphic() || b == b' '
}

/// A count of cells as a float; a line of more than 65,535 cells is drawn that wide.
fn cells(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::{Advance, Put, Shown, word_at};

    type Check = Result<(), Box<dyn std::error::Error>>;

    const ADV: Advance = Advance {
        code: 6.0,
        hint: 5.0,
    };

    const NONE: &[(u32, &str)] = &[];

    fn never(_: usize) -> f32 {
        f32::NAN
    }

    #[test]
    fn a_hint_between_two_tokens_shifts_what_follows_and_is_never_hit() -> Check {
        let source = "Some(left) => {";
        let shown = Shown::new(source, &[(9, ": u32")]);
        assert_eq!(shown.text(), "Some(left : u32 ) => {");
        assert_eq!(shown.puts(), [(9..16, Put::Hint)]);
        // `left` keeps its place; `)` moves right by the hint's seven narrower cells.
        assert_eq!(shown.x_of(shown.to_drawn(5), ADV, never), 30.0);
        assert_eq!(
            shown.x_of(shown.to_drawn(9), ADV, never),
            9.0 * 6.0 + 7.0 * 5.0
        );
        // A point over the hint is not source; a point over `)` after it is source byte 9.
        assert_eq!(
            shown
                .at_x(9.0 * 6.0 + 12.0, ADV, never)
                .map(|at| shown.to_source(at)),
            Some(None)
        );
        assert_eq!(
            shown
                .at_x(9.0 * 6.0 + 35.0 + 1.0, ADV, never)
                .map(|at| shown.to_source(at)),
            Some(Some(9))
        );
        // The word under a point over the `f` of `left` is `left`, in source bytes.
        let at = shown
            .at_x(7.5 * 6.0, ADV, never)
            .and_then(|at| shown.to_source(at));
        assert_eq!(at.and_then(|col| word_at(source, col)), Some(5..9));
        // A range ending where the hint goes in stops before it.
        assert_eq!(shown.drawn(source, 5..9), Some(5..9));
        assert_eq!(shown.at_x(22.0 * 6.0, ADV, never), None);
        Ok(())
    }

    #[test]
    fn a_tab_runs_to_its_stop_and_maps_back_to_one_byte() -> Check {
        let shown = Shown::new("\tlet\tx = \t1", NONE);
        assert_eq!(shown.text(), "    let x =     1");
        assert_eq!(shown.puts(), [(1..4, Put::TabStop), (13..16, Put::TabStop)]);
        assert_eq!(shown.to_drawn(1), 4);
        assert_eq!(
            (
                shown.to_source(0),
                shown.to_source(2),
                shown.to_source(4),
                shown.to_source(8)
            ),
            (Some(0), None, Some(1), Some(5))
        );
        assert_eq!(shown.x_of(shown.to_drawn(5), ADV, never), 48.0);
        assert_eq!(
            shown
                .at_x(16.0 * 6.0 + 1.0, ADV, never)
                .map(|at| shown.to_source(at)),
            Some(Some(10))
        );
        // A selection from inside the first tab's stop to the middle of the second copies from `l`.
        assert_eq!((shown.source_before(2), shown.source_before(14)), (1, 10));
        Ok(())
    }

    #[test]
    fn a_line_with_wide_characters_is_measured_not_counted() -> Check {
        let source = "let 名前 = 1;";
        let shown = Shown::new(source, NONE);
        // A stand-in for the shaper: an ASCII cell is 6 points, any other character 12.
        let measured = |at: usize| {
            let before = source.get(..at).unwrap_or(source);
            before
                .chars()
                .map(|c| if c.is_ascii() { 6.0 } else { 12.0 })
                .sum::<f32>()
        };
        assert_eq!(shown.x_of(10, ADV, measured), 4.0 * 6.0 + 2.0 * 12.0);
        assert_eq!(shown.at_x(30.0, ADV, measured), Some(4));
        assert_eq!(shown.at_x(47.0, ADV, measured), Some(7));
        assert_eq!(shown.at_x(48.0, ADV, measured), Some(10));
        assert_eq!(shown.at_x(4000.0, ADV, measured), None);
        assert_eq!(word_at(source, 7), Some(4..10));
        Ok(())
    }

    #[test]
    fn a_hint_inside_a_character_or_past_the_line_is_left_out() -> Check {
        let shown = Shown::new("é = 1", &[(1, ": a"), (5, ": b"), (40, ": c")]);
        assert_eq!(shown.text(), "é =  : b 1");
        assert_eq!(Shown::new("ab", &[(2, ": end")]).text(), "ab : end ");
        Ok(())
    }
}
