//! Lines and slots as virtual rows, and how a line is named: its key and its thing.

use std::sync::Arc;

use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::kind::Kind;

use super::doc::{Doc, LineNo};

/// The most bytes of code a thing carries as its excerpt, in whole lines.
const EXCERPT: usize = 1024;

/// A run of whole lines, first to last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lines {
    first: LineNo,
    last: LineNo,
}

impl Lines {
    /// The lines from `a` to `b`, whichever order they come in.
    pub fn new(a: LineNo, b: LineNo) -> Lines {
        Lines {
            first: a.min(b),
            last: a.max(b),
        }
    }

    pub fn first(self) -> LineNo {
        self.first
    }

    pub fn last(self) -> LineNo {
        self.last
    }

    pub fn contains(self, line: LineNo) -> bool {
        (self.first..=self.last).contains(&line)
    }
}

/// The key of a line's row on the surface keyed `prefix`: `<prefix>.L<n>`.
pub fn line_key(prefix: &str, line: LineNo) -> String {
    [prefix, ".L", &line.get().to_string()].concat()
}

/// The line a picked key belongs to: the row itself or anything keyed inside it.
pub fn line_of(key: &str) -> Option<LineNo> {
    let number = |part: &str| part.strip_prefix('L')?.parse::<u32>().ok();
    LineNo::try_from(key.rsplit('.').find_map(number)?).ok()
}

/// What `lines` of the file at `path` stand for: `yi://lines/<path>#L13-L20`, named by the
/// file's name, with their text up to the excerpt's cap. Nothing for lines the text lacks.
pub fn thing(path: &str, doc: &Doc, lines: Lines) -> Option<Thing> {
    if lines.last().index() >= doc.lines() {
        return None;
    }
    let (first, last) = (lines.first().get(), lines.last().get());
    let (part, range) = if first == last {
        (format!("L{first}"), first.to_string())
    } else {
        (format!("L{first}-L{last}"), format!("{first}-{last}"))
    };
    let name = path.rsplit('/').next().unwrap_or(path);
    let fits = doc.lines_within(lines.first().index(), EXCERPT);
    let end = fits.min(lines.last().index().saturating_add(1));
    let excerpt = doc.slice(lines.first().index(), end).trim_end();
    // One line can be longer than the cap: it is cut after its last whole character inside it.
    let ends = excerpt
        .char_indices()
        .map(|(at, c)| at.saturating_add(c.len_utf8()));
    let cut = ends.take_while(|end| *end <= EXCERPT).last().unwrap_or(0);
    let kept = excerpt.get(..cut).unwrap_or(excerpt);
    let asked = doc.slice(
        lines.first().index(),
        lines.last().index().saturating_add(1),
    );
    let total = asked.trim_end().len();
    let excerpt = if kept.len() < total {
        let cut = format!("[…] kept {cut} of {total} bytes of these lines (cap 1 KiB)");
        format!("{kept}\n{cut}; read {path} for the rest")
    } else {
        kept.to_owned()
    };
    Some(Thing {
        label: format!("{name}:{range}"),
        address: Address::new(Kind::Lines, path).ok()?.part(part),
        excerpt: (!excerpt.is_empty()).then_some(excerpt),
    })
}

/// The file a code pane shows and the lines its viewport holds, kept for the pane to be sent as
/// words.
#[derive(Default)]
pub struct InView(pub Option<(String, Arc<Doc>, Lines)>);

impl InView {
    /// The path, which lines of how many, and their text.
    pub fn words(&self) -> Option<String> {
        let (path, doc, lines) = self.0.as_ref()?;
        let end = lines.last().index().saturating_add(1);
        let (first, last) = (lines.first().get(), lines.last().get());
        let text = doc.slice(lines.first().index(), end).trim_end();
        let total = doc.lines();
        Some(format!(
            "{path}, lines {first}-{last} of {total} in view:\n{text}"
        ))
    }
}

/// What one row of the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The line at this 0-based index.
    Line(usize),
    /// The slot at this index of the slots given.
    Slot(usize),
}

/// What row `row` shows in a text of `lines` lines with a slot after each of the 0-based lines
/// in `after`, which is in order. Nothing past the last row.
pub fn row_at(lines: usize, after: &[usize], row: usize) -> Option<Row> {
    // The slot at index j sits under its line and under the j slots before it.
    let place = |j: usize| {
        after.get(j).map(|line| {
            line.min(&lines.saturating_sub(1))
                .saturating_add(j)
                .saturating_add(1)
        })
    };
    let before = (0..after.len())
        .take_while(|&j| place(j).is_some_and(|at| at < row))
        .count();
    if place(before) == Some(row) && lines > 0 {
        return Some(Row::Slot(before));
    }
    let line = row.saturating_sub(before);
    (line < lines).then_some(Row::Line(line))
}

/// How many rows that text has.
pub fn row_count(lines: usize, slots: usize) -> usize {
    if lines == 0 {
        0
    } else {
        lines.saturating_add(slots)
    }
}

#[cfg(test)]
mod tests {
    use super::{Lines, Row, line_key, line_of, row_at, row_count, thing};
    use crate::code::doc::{Doc, Lang, LineNo};

    type Check = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_slot_sits_under_its_line_and_pushes_the_lines_below_down_one_row() -> Check {
        let rows = |after: &[usize]| {
            (0..row_count(4, after.len()) + 1)
                .map(|r| row_at(4, after, r))
                .collect::<Vec<_>>()
        };
        let line = |i| Some(Row::Line(i));
        assert_eq!(rows(&[]), [line(0), line(1), line(2), line(3), None]);
        assert_eq!(
            rows(&[1]),
            [line(0), line(1), Some(Row::Slot(0)), line(2), line(3), None]
        );
        assert_eq!(
            rows(&[0, 0, 3]),
            [
                line(0),
                Some(Row::Slot(0)),
                Some(Row::Slot(1)),
                line(1),
                line(2),
                line(3),
                Some(Row::Slot(2)),
                None
            ]
        );
        // A slot after a line the text does not have goes under the last line.
        assert_eq!(
            rows(&[9]),
            [line(0), line(1), line(2), line(3), Some(Row::Slot(0)), None]
        );
        assert_eq!(row_at(0, &[0], 0), None);
        Ok(())
    }

    #[test]
    fn a_line_is_found_again_from_its_key_and_from_any_key_inside_it() -> Check {
        let line = LineNo::try_from(13)?;
        assert_eq!(line_key("code-3", line), "code-3.L13");
        assert_eq!(line_of("code-3.L13"), Some(line));
        assert_eq!(line_of("code-3.L13.value"), Some(line));
        assert_eq!(line_of("code-3.row.L13"), Some(line));
        assert_eq!(line_of("code-3.pane-header"), None);
        assert_eq!(line_of("code-3.L0"), None);
        Ok(())
    }

    #[test]
    fn lines_stand_for_an_address_with_their_file_and_range() -> Check {
        let doc = Doc::new("use a;\n\nfn take() {\n    1\n}\n", Lang::RUST, 0);
        let (three, five) = (LineNo::try_from(3)?, LineNo::try_from(5)?);
        let one = thing(
            "src/middleware/rate_limit.rs",
            &doc,
            Lines::new(three, three),
        )
        .ok_or("no thing for a line")?;
        assert_eq!(
            one.address.to_string(),
            "yi://lines/src/middleware/rate_limit.rs#L3"
        );
        assert_eq!(one.label, "rate_limit.rs:3");
        assert_eq!(one.excerpt.as_deref(), Some("fn take() {"));
        let run = thing(
            "src/middleware/rate_limit.rs",
            &doc,
            Lines::new(five, three),
        )
        .ok_or("no thing for a range")?;
        assert_eq!(
            run.address.to_string(),
            "yi://lines/src/middleware/rate_limit.rs#L3-L5"
        );
        assert_eq!(run.label, "rate_limit.rs:3-5");
        assert_eq!(run.excerpt.as_deref(), Some("fn take() {\n    1\n}"));
        assert!(thing("", &doc, Lines::new(three, three)).is_none());
        let six = LineNo::try_from(6)?;
        assert!(
            thing("a.rs", &doc, Lines::new(three, six)).is_none(),
            "a line the text does not have stands for nothing"
        );
        let long = Doc::new("é".repeat(4000), Lang::PLAIN, 0);
        let one = LineNo::try_from(1)?;
        let cut = thing("a.txt", &long, Lines::new(one, one)).ok_or("no thing")?;
        let said = cut.excerpt.ok_or("no excerpt")?;
        let (kept, notice) = said.split_once("\n[…]").ok_or("no notice at the cut")?;
        assert_eq!(kept.len(), 1024);
        assert_eq!(
            notice,
            " kept 1024 of 8000 bytes of these lines (cap 1 KiB); read a.txt for the rest"
        );
        Ok(())
    }
}
