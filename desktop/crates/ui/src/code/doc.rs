//! A document: text, line starts, language, revision.

use std::num::{NonZeroU32, TryFromIntError};
use std::ops::Range;

/// A line of a [`Doc`], counted from 1 as editors and language servers show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineNo(NonZeroU32);

impl LineNo {
    /// The line at 0-based `index`; an index past `u32::MAX` names the last countable line.
    pub fn from_index(index: usize) -> LineNo {
        let n = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        LineNo(NonZeroU32::new(n).unwrap_or(NonZeroU32::MIN))
    }

    /// The 0-based index of this line.
    pub fn index(self) -> usize {
        usize::try_from(self.0.get()).map_or(usize::MAX, |n| n.saturating_sub(1))
    }

    /// The number as shown in the gutter.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for LineNo {
    type Error = TryFromIntError;

    fn try_from(n: u32) -> Result<LineNo, TryFromIntError> {
        NonZeroU32::try_from(n).map(LineNo)
    }
}

/// A byte range inside one line's text, counted from the line's first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cols {
    start: u32,
    end: u32,
}

impl Cols {
    /// The bytes between `a` and `b`, whichever order they come in.
    pub fn new(a: u32, b: u32) -> Cols {
        Cols {
            start: a.min(b),
            end: a.max(b),
        }
    }

    pub fn start(self) -> u32 {
        self.start
    }

    pub fn end(self) -> u32 {
        self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn contains(self, col: u32) -> bool {
        (self.start..self.end).contains(&col)
    }

    /// This range as indices into `line`, or nothing when it is empty, runs past the line or
    /// cuts a character: a mark made for an older text is dropped, never drawn in the wrong place.
    pub fn within(self, line: &str) -> Option<Range<usize>> {
        let range = usize::try_from(self.start).ok()?..usize::try_from(self.end).ok()?;
        (!self.is_empty() && line.get(range.clone()).is_some()).then_some(range)
    }
}

/// A language the surface colours, by the name Rune's highlighter knows it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lang(Option<&'static str>);

impl Lang {
    pub const PLAIN: Lang = Lang(None);
    pub const RUST: Lang = Lang(Some("rust"));
    pub const TYPESCRIPT: Lang = Lang(Some("typescript"));
    pub const PYTHON: Lang = Lang(Some("python"));
    pub const TOML: Lang = Lang(Some("toml"));
    pub const SHELL: Lang = Lang(Some("bash"));
    pub const JSON: Lang = Lang(Some("json"));

    /// The language of the file at `path`, as Rune names it from the file's name.
    pub fn for_path(path: &str) -> Lang {
        Lang(rune_kit::syntax::lang_for_path(path))
    }

    /// The highlighter's name for it; none for plain text.
    pub fn name(self) -> Option<&'static str> {
        self.0
    }

    /// The name a header shows: `Rust`, or the highlighter's own name for the rest.
    pub fn title(self) -> &'static str {
        match self.0 {
            Some("rust") => "Rust",
            Some("typescript") => "TypeScript",
            Some("python") => "Python",
            Some("toml") => "TOML",
            // Incident: Rune names a `.sh` file `zsh`, not `bash`.
            Some("bash" | "zsh" | "sh") => "Shell",
            Some("json") => "JSON",
            Some(other) => other,
            None => "Text",
        }
    }
}

/// A source text split into lines. Line ends (`\n`, `\r\n`) are not part of a line's text, and
/// a final line end starts no extra line.
#[derive(Debug, Clone)]
pub struct Doc {
    text: String,
    /// The byte each line starts at, then the text's length.
    starts: Vec<usize>,
    /// The widest line, in cells: a tab is four and anything past ASCII two.
    widest: usize,
    lang: Lang,
    rev: u64,
}

impl Doc {
    pub fn new(text: impl Into<String>, lang: Lang, rev: u64) -> Doc {
        let text = text.into();
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i.saturating_add(1)));
        if starts.last() != Some(&text.len()) {
            starts.push(text.len());
        }
        let cells = |line: &str| {
            let wide = |c: char| match c {
                '\t' => 4,
                c if c.is_ascii() => 1,
                _ => 2,
            };
            line.chars().map(wide).fold(0usize, usize::saturating_add)
        };
        let widest = text.split('\n').map(cells).max().unwrap_or(0);
        Doc {
            text,
            starts,
            widest,
            lang,
            rev,
        }
    }

    /// An upper bound on the widest line, in character cells: how far the text scrolls sideways.
    pub fn widest(&self) -> usize {
        self.widest
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// The caller's revision of the text: rows are built again when it changes.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn lines(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    /// The byte range of 0-based lines `from..to` with their line ends, clamped to the text.
    pub fn span(&self, from: usize, to: usize) -> Range<usize> {
        let at = |i: usize| self.starts.get(i).copied().unwrap_or(self.text.len());
        at(from)..at(to).max(at(from))
    }

    /// The end of the longest run of whole lines from 0-based `from` that fits `limit` bytes:
    /// always at least one line past `from`, never past the last line.
    pub fn lines_within(&self, from: usize, limit: usize) -> usize {
        let cut = self.span(from, from).start.saturating_add(limit);
        let fit = self.starts.partition_point(|&s| s <= cut).saturating_sub(1);
        fit.max(from.saturating_add(1)).min(self.lines())
    }

    /// The text of 0-based lines `from..to`, line ends included.
    pub fn slice(&self, from: usize, to: usize) -> &str {
        self.text.get(self.span(from, to)).unwrap_or("")
    }

    /// One line's text without its line end; nothing for a line the text does not have.
    pub fn line(&self, line: LineNo) -> Option<&str> {
        let i = line.index();
        (i < self.lines()).then(|| {
            let whole = self.slice(i, i.saturating_add(1));
            let whole = whole.strip_suffix('\n').unwrap_or(whole);
            whole.strip_suffix('\r').unwrap_or(whole)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cols, Doc, Lang, LineNo};

    type Check = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_final_line_end_starts_no_extra_line_and_crlf_is_not_text() -> Check {
        let doc = Doc::new("a\r\nbb\n\nlast\n", Lang::PLAIN, 0);
        assert_eq!(doc.lines(), 4);
        let lines: Vec<_> = (0..5).map(|i| doc.line(LineNo::from_index(i))).collect();
        assert_eq!(lines, [Some("a"), Some("bb"), Some(""), Some("last"), None]);
        assert_eq!(
            Doc::new("no end", Lang::PLAIN, 0).line(LineNo::try_from(1)?),
            Some("no end")
        );
        assert_eq!(Doc::new("", Lang::PLAIN, 0).lines(), 0);
        Ok(())
    }

    #[test]
    fn the_widest_line_counts_a_tab_as_four_cells_and_a_wide_character_as_two() -> Check {
        assert_eq!(Doc::new("ab\n\tabc\nx", Lang::PLAIN, 0).widest(), 7);
        assert_eq!(Doc::new("let 名前 = 1;\n", Lang::PLAIN, 0).widest(), 13);
        assert_eq!(
            Lang::for_path("src/middleware/rate_limit.rs").title(),
            "Rust"
        );
        assert_eq!(Lang::for_path("notes.unknown").title(), "Text");
        let named = [
            ("src/app.ts", "TypeScript"),
            ("tools/gate.py", "Python"),
            ("Cargo.toml", "TOML"),
            ("scripts/run.sh", "Shell"),
            ("package.json", "JSON"),
        ];
        for (path, title) in named {
            assert_eq!(Lang::for_path(path).title(), title, "{path}");
        }
        Ok(())
    }

    #[test]
    fn a_range_that_cuts_a_character_or_leaves_the_line_is_dropped() -> Check {
        let line = "let é = 1;";
        assert_eq!(Cols::new(4, 6).within(line), Some(4..6));
        assert_eq!(Cols::new(6, 4).within(line), Some(4..6));
        assert_eq!(Cols::new(4, 5).within(line), None);
        assert_eq!(Cols::new(8, 40).within(line), None);
        assert_eq!(Cols::new(3, 3).within(line), None);
        Ok(())
    }
}
