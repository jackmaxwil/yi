//! One highlighter for a short text shown outside the code viewer (a chat step, a kernel cell):
//! Rune's lexers by language, Yi's numbered rows, a diff, a Python traceback and ANSI colour.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, El, Span, TextHighlight, TextStyle, rich};
use rune_kit::syntax::{Paint, Syntax, builtin};
use yd_core::settings::store::value::Json;
use yd_core::tokens::Tokens;

use super::doc::Lang;
use super::view::Look;
use crate::term::screen::Palette;
use crate::theme::look::colour;
use crate::theme::scale::CODE_MATCH;

/// Invariant: Rune's lexers colour the first 128 KiB of a source, cut at a line end; this
/// highlighter stops there too and says so ([`Painted::notice`]).
pub const CAP: usize = 128 * 1024;

/// Texts kept painted: every step a conversation shows open, and more.
const KEPT: usize = 64;

/// How a text is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Read {
    /// Source in one language.
    Code(Lang),
    /// Yi's `read` and `grep` rows: `[path#TAG]` heads, then `N:text`, each file's text in the
    /// language its name says. With `hits`, a `N:` row is a search hit and is marked.
    Numbered { hits: bool },
    /// A unified diff: added and removed lines tinted.
    Diff,
    /// A Python traceback: its files and lines tinted, the error named in the error colour.
    Traceback,
    /// What a command printed: coloured only when it is clearly JSON, a diff or a Python
    /// traceback, else plain. ANSI colour in it is drawn in either case.
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ink {
    Syntax(Syntax),
    Faint,
    Added,
    Removed,
    Hunk,
    File,
    Line,
    Error,
    Term(vt100::Color),
}

/// Where colour stopped, as 1-based lines: the line it stopped before, and how many there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop {
    pub line: usize,
    pub lines: usize,
}

/// A text made ready to draw: control and escape codes gone, colour runs over what is left.
#[derive(Debug, Default)]
pub struct Painted {
    text: String,
    runs: Vec<(Range<usize>, Ink)>,
    hits: Vec<Range<usize>>,
    stopped: Option<Stop>,
}

thread_local! {
    /// Invariant: at most `KEPT` texts, oldest first; a view cannot keep one itself.
    static KEPT_TEXTS: RefCell<VecDeque<(u64, Rc<Painted>)>> = const { RefCell::new(VecDeque::new()) };
}

/// `text` read as `read`, painted once and found again while it is the same text.
pub fn paint(text: &str, read: Read) -> Rc<Painted> {
    let mut hasher = DefaultHasher::new();
    (text, read).hash(&mut hasher);
    let digest = hasher.finish();
    KEPT_TEXTS.with(|kept| {
        let Ok(mut kept) = kept.try_borrow_mut() else {
            return Rc::new(Painted::of(text, read));
        };
        if let Some((_, found)) = kept.iter().find(|(d, _)| *d == digest) {
            return Rc::clone(found);
        }
        let made = Rc::new(Painted::of(text, read));
        if kept.len() >= KEPT {
            kept.pop_front();
        }
        kept.push_back((digest, Rc::clone(&made)));
        made
    })
}

impl Painted {
    fn of(source: &str, read: Read) -> Painted {
        let (text, ansi) = clean(source);
        let cut = coloured_len(&text);
        let mut out = Painted {
            stopped: (cut < text.len()).then(|| Stop {
                line: text.get(..cut).map_or(0, |t| t.matches('\n').count()) + 1,
                lines: text.lines().count(),
            }),
            ..Painted::default()
        };
        let head = text.get(..cut).unwrap_or("");
        if ansi.is_empty() {
            out.read(head, read);
        } else {
            out.runs = ansi.into_iter().filter(|(r, _)| r.end <= cut).collect();
        }
        out.text = text;
        out
    }

    fn read(&mut self, head: &str, read: Read) {
        match read {
            Read::Code(lang) => self.lex(head, lang, &[(0, head.len())]),
            Read::Numbered { hits } => self.numbered(head, hits),
            Read::Diff => self.diff(head),
            Read::Traceback => self.traceback(head),
            Read::Output => match guess(head) {
                Some(Read::Output) | None => {}
                Some(found) => self.read(head, found),
            },
        }
    }

    /// Lexes the parts of `text` named by `(start, end)` as one source, joined by line ends.
    fn lex(&mut self, text: &str, lang: Lang, parts: &[(usize, usize)]) {
        let mut at = Vec::with_capacity(parts.len());
        let mut source = String::new();
        for &(a, b) in parts {
            at.push((source.len(), a, b.saturating_sub(a)));
            source.push_str(text.get(a..b).unwrap_or(""));
            source.push('\n');
        }
        let mut first = 0usize;
        for (range, paint) in builtin(lang.name(), &source) {
            let Paint::Kind(kind) = paint else { continue };
            while at
                .get(first)
                .is_some_and(|&(f, _, len)| f.saturating_add(len) < range.start)
            {
                first = first.saturating_add(1);
            }
            // A run may cross line ends (a block comment): each part gets its own piece.
            for &(f, a, len) in at.iter().skip(first).take_while(|(f, _, _)| *f < range.end) {
                let s = range.start.max(f).saturating_sub(f);
                let e = range.end.min(f.saturating_add(len)).saturating_sub(f);
                if s < e {
                    self.runs
                        .push((a.saturating_add(s)..a.saturating_add(e), Ink::Syntax(kind)));
                }
            }
        }
    }

    fn numbered(&mut self, text: &str, hits: bool) {
        let mut lang = Lang::PLAIN;
        let mut rows: Vec<(usize, usize)> = Vec::new();
        for (start, line) in lines(text) {
            let end = start.saturating_add(line.len());
            if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                self.lex(text, lang, &rows);
                rows.clear();
                lang = Lang::for_path(head.rsplit_once('#').map_or(head, |(path, _)| path));
                self.runs.push((start..end, Ink::Faint));
                continue;
            }
            let digits = leading_digits(line);
            let sep = line.get(digits..digits.saturating_add(1));
            match (digits, sep) {
                (1.., Some(":" | "-")) => {
                    let body = start.saturating_add(digits).saturating_add(1);
                    self.runs.push((start..body, Ink::Faint));
                    rows.push((body, end));
                    if hits && sep == Some(":") && body < end {
                        self.hits.push(body..end);
                    }
                }
                _ => self.runs.push((start..end, Ink::Faint)),
            }
        }
        self.lex(text, lang, &rows);
        self.runs.sort_by_key(|(r, _)| r.start);
    }

    fn diff(&mut self, text: &str) {
        for (start, line) in lines(text) {
            let ink = match line.as_bytes().first() {
                _ if ["+++", "---", "diff ", "index "]
                    .iter()
                    .any(|h| line.starts_with(h)) =>
                {
                    Ink::Faint
                }
                _ if line.starts_with("@@") => Ink::Hunk,
                Some(b'+') => Ink::Added,
                Some(b'-') => Ink::Removed,
                _ => continue,
            };
            self.runs
                .push((start..start.saturating_add(line.len()), ink));
        }
    }

    fn traceback(&mut self, text: &str) {
        for (start, line) in lines(text) {
            let mut put = |from: usize, to: usize, ink: Ink| {
                self.runs
                    .push((start.saturating_add(from)..start.saturating_add(to), ink));
            };
            let digits = |from: usize| line.get(from..).map_or(0, leading_digits);
            // `File "x.py", line 3, in f`, and IPython's `Cell In[4], line 3`.
            let file = match (line.find("File \""), line.find("Cell In[")) {
                (Some(a), _) => line
                    .find("\", line ")
                    .map(|q| (a.saturating_add(5), q.saturating_add(1))),
                (None, Some(a)) => line.find("], line ").map(|q| (a, q.saturating_add(1))),
                _ => None,
            };
            if let Some((a, b)) = file {
                let n = b.saturating_add(2);
                put(a, b, Ink::File);
                put(
                    n,
                    n.saturating_add(5)
                        .saturating_add(digits(n.saturating_add(5))),
                    Ink::Line,
                );
                continue;
            }
            let name = line.split([':', ' ']).next().unwrap_or("");
            let named = name.ends_with("Error") || name.ends_with("Exception");
            if named
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                put(0, name.len(), Ink::Error);
                continue;
            }
            // IPython's arrow at the line that raised: `----> 3 ratio(1, 0)`.
            let lead = line.len().saturating_sub(line.trim_start().len());
            let dashes = line
                .get(lead..)
                .map_or(0, |t| t.len() - t.trim_start_matches('-').len());
            let tip = lead.saturating_add(dashes);
            if dashes >= 2 && line.get(tip..).is_some_and(|t| t.starts_with('>')) {
                let after = tip.saturating_add(1);
                let gap = line
                    .get(after..)
                    .map_or(0, |t| t.len() - t.trim_start().len());
                let n = after.saturating_add(gap);
                put(lead, n.saturating_add(digits(n)), Ink::Error);
            }
        }
    }

    /// The text to draw.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where colour stopped before the text's end, past [`CAP`].
    pub fn stopped(&self) -> Option<Stop> {
        self.stopped
    }

    /// The `[…]` line that says where colour stopped, for the row that draws the text.
    pub fn notice(&self) -> Option<String> {
        self.stopped.map(|Stop { line, lines }| {
            format!(
                "[…] colour stops at line {line} of {lines}: the highlighter colours the first {} KiB, the rest is plain",
                CAP / 1024
            )
        })
    }

    /// The search hits, as byte ranges of [`Painted::text`].
    pub fn hits(&self) -> &[Range<usize>] {
        &self.hits
    }

    /// The colour runs as spans, in the theme's code, diff and terminal colours.
    pub fn spans(&self, tokens: &Tokens) -> Vec<Span> {
        let look = Look::of(tokens);
        let palette = Palette::new(tokens);
        let run = |(range, ink): &(Range<usize>, Ink)| {
            let span = Span::new(range.start, range.end);
            let paint = |c: Color| Some(span.clone().color(c));
            match ink {
                Ink::Syntax(kind) => paint(look.syntax.color(*kind)),
                Ink::Faint => paint(look.faint),
                Ink::Added => paint(look.ok).map(|s| s.bg(colour(tokens.added), 0.0)),
                Ink::Removed => paint(look.bad).map(|s| s.bg(colour(tokens.removed), 0.0)),
                Ink::Hunk => paint(colour(tokens.type_name)),
                Ink::File => paint(colour(tokens.string)),
                Ink::Line => paint(colour(tokens.number)),
                Ink::Error => paint(look.bad),
                Ink::Term(c) => palette.rgb(*c).and_then(paint),
            }
        };
        self.runs.iter().filter_map(run).collect()
    }

    /// The text as one element keyed `key`: its colours, and each hit marked and keyed
    /// `<key>.hit.<n>`.
    pub fn draw(&self, tokens: &Tokens, key: &str, style: TextStyle, ink: Color) -> El {
        let lit = colour(CODE_MATCH.of(tokens).alpha(0.32));
        let marks =
            self.hits.iter().enumerate().map(|(n, range)| {
                TextHighlight::new(range.clone(), lit).key(format!("{key}.hit.{n}"))
            });
        rich(self.text.clone(), self.spans(tokens))
            .key(key.to_owned())
            .font(style)
            .color(ink)
            .text_highlights(marks)
    }
}

/// What a command's output clearly is, if anything.
fn guess(text: &str) -> Option<Read> {
    let body = text.trim();
    let starts = |head: &str| text.lines().any(|l| l.starts_with(head));
    if body.starts_with(['{', '[']) && body.parse::<Json>().is_ok() {
        Some(Read::Code(Lang::JSON))
    } else if starts("@@ ") && starts("--- ") {
        Some(Read::Diff)
    } else if text.contains("Traceback (most recent call last)") {
        Some(Read::Traceback)
    } else {
        None
    }
}

fn leading_digits(text: &str) -> usize {
    text.len()
        .saturating_sub(text.trim_start_matches(|c: char| c.is_ascii_digit()).len())
}

/// Each line of `text` with the byte it starts at, line ends left out.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split('\n').scan(0usize, |at, line| {
        let start = *at;
        *at = start.saturating_add(line.len()).saturating_add(1);
        Some((start, line.strip_suffix('\r').unwrap_or(line)))
    })
}

/// How much of `text` gets colour: all of it, or through the last line end within [`CAP`].
fn coloured_len(text: &str) -> usize {
    if text.len() <= CAP {
        return text.len();
    }
    let head = text.as_bytes().get(..CAP).unwrap_or_default();
    head.iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |end| end.saturating_add(1))
}

/// `source` with escape sequences, control and direction characters gone and a tab as four
/// spaces, and the runs its SGR codes coloured.
fn clean(source: &str) -> (String, Vec<(Range<usize>, Ink)>) {
    let bidi = |c: char| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let mut out = String::with_capacity(source.len());
    let mut runs = Vec::new();
    let mut ink: Option<(usize, vt100::Color)> = None;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                let next = match chars.next_if(|&n| n == '[' || n == ']') {
                    Some('[') => {
                        let mut params = String::new();
                        let end = chars.by_ref().find(|&p| {
                            let done = ('\u{40}'..='\u{7e}').contains(&p);
                            if !done {
                                params.push(p);
                            }
                            done
                        });
                        (end == Some('m')).then(|| sgr(&params, ink.map(|(_, c)| c)))
                    }
                    // An OSC runs to BEL or to ESC `\`.
                    Some(_) => {
                        let _ = chars.by_ref().find(|&p| p == '\u{7}' || p == '\u{1b}');
                        let _ = chars.next_if_eq(&'\\');
                        None
                    }
                    None => {
                        let _ = chars.next();
                        None
                    }
                };
                if let Some(next) = next
                    && next != ink.map(|(_, c)| c)
                {
                    if let Some((from, c)) = ink.take()
                        && from < out.len()
                    {
                        runs.push((from..out.len(), Ink::Term(c)));
                    }
                    ink = next.map(|c| (out.len(), c));
                }
            }
            '\t' => out.push_str("    "),
            '\n' => out.push('\n'),
            c if c.is_control() || bidi(c) => {}
            c => out.push(c),
        }
    }
    if let Some((from, c)) = ink
        && from < out.len()
    {
        runs.push((from..out.len(), Ink::Term(c)));
    }
    (out, runs)
}

/// The foreground an SGR sequence leaves, from `was`: `None` is the text's own colour.
fn sgr(params: &str, was: Option<vt100::Color>) -> Option<vt100::Color> {
    let mut codes = params.split(';').map(|p| p.parse::<u16>().unwrap_or(0));
    let mut now = was;
    while let Some(code) = codes.next() {
        let byte = |n: Option<u16>| u8::try_from(n.unwrap_or(0)).unwrap_or(0);
        now = match code {
            0 | 39 => None,
            30..=37 => Some(vt100::Color::Idx(byte(Some(code - 30)))),
            90..=97 => Some(vt100::Color::Idx(byte(Some(code - 82)))),
            38 => match codes.next() {
                Some(5) => Some(vt100::Color::Idx(byte(codes.next()))),
                Some(2) => {
                    let (r, g, b) = (codes.next(), codes.next(), codes.next());
                    Some(vt100::Color::Rgb(byte(r), byte(g), byte(b)))
                }
                _ => now,
            },
            _ => now,
        };
    }
    now
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::{CAP, Ink, Painted, Read, Stop, paint};
    use crate::code::doc::Lang;
    use crate::code::view::sample;

    type Check = Result<(), Box<dyn std::error::Error>>;

    /// A Python text of exactly `len` bytes whose last byte is a line end.
    fn python(len: usize) -> Result<String, Box<dyn std::error::Error>> {
        let mut text = sample::PYTHON.repeat(len / sample::PYTHON.len() + 1);
        let cut = text
            .get(..len.saturating_sub(1))
            .ok_or("short")?
            .rfind('\n')
            .ok_or("no line")?;
        text = text.get(..=cut).ok_or("cut")?.to_owned();
        text.push_str(&"#".repeat(len.saturating_sub(text.len()).saturating_sub(1)));
        text.push('\n');
        Ok(text)
    }

    #[test]
    fn colour_reaches_the_cap_and_past_it_stops_at_a_line_end_and_says_where() -> Check {
        let at = Painted::of(&python(CAP)?, Read::Code(Lang::PYTHON));
        assert_eq!(at.text().len(), CAP);
        assert_eq!((at.stopped(), at.notice()), (None, None));
        let past = python(CAP)? + "x = 1\n";
        let over = Painted::of(&past, Read::Code(Lang::PYTHON));
        let lines = past.lines().count();
        assert_eq!(over.stopped(), Some(Stop { line: lines, lines }));
        let notice = over.notice().ok_or("no notice")?;
        assert!(notice.starts_with(&format!("[…] colour stops at line {lines} of {lines}")));
        let end = over.runs.iter().map(|(r, _)| r.end).max().unwrap_or(0);
        assert!(end <= CAP, "a run at {end} is past the cap");
        assert!(
            over.runs.iter().any(|(r, _)| r.end > CAP - 4096),
            "colour runs up to the cap"
        );
        Ok(())
    }

    #[test]
    fn a_text_is_painted_once_while_it_stays_the_same() -> Check {
        let first = paint("ls -la\n", Read::Code(Lang::SHELL));
        assert!(Rc::ptr_eq(
            &first,
            &paint("ls -la\n", Read::Code(Lang::SHELL))
        ));
        assert!(!Rc::ptr_eq(&first, &paint("ls -la\n", Read::Output)));
        Ok(())
    }

    #[test]
    fn sgr_codes_colour_runs_and_every_other_escape_is_dropped() -> Check {
        let painted = Painted::of(
            "\u{1b}]0;title\u{7}\u{1b}[38;5;196mX\u{1b}[38;2;1;2;3mY\u{1b}[0mZ\u{1b}[2K\r\n",
            Read::Output,
        );
        assert_eq!(painted.text(), "XYZ\n");
        let runs = [
            (0..1, Ink::Term(vt100::Color::Idx(196))),
            (1..2, Ink::Term(vt100::Color::Rgb(1, 2, 3))),
        ];
        assert_eq!(painted.runs, runs);
        Ok(())
    }

    #[test]
    fn output_is_coloured_only_when_it_is_wholly_one_language() -> Check {
        let json = Painted::of("{\"rows\": 3, \"ok\": true}\n", Read::Output);
        assert!(json.runs.iter().any(|(_, ink)| *ink != Ink::Faint));
        let mixed = Painted::of(
            "{\"rows\": 3, \"ok\": true}\n\nhelp: rlm.run(prompt)\n",
            Read::Output,
        );
        assert_eq!(mixed.runs, []);
        let words = Painted::of("Compiling tally v0.1.0\n--- fine\n", Read::Output);
        assert_eq!(words.runs, []);
        Ok(())
    }
}
