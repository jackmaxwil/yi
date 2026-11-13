use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::{Over, Theme};
use crate::highlight::{self, Lang};

/// `show_controls` stores ESC as its control picture, so colour codes stay inert until decoded.
const ESC: char = '\u{241b}';
const BEL: char = '\u{2407}';

/// Invariant: a break in the shown text (a gap, an elision, a clipped row, a parse error)
/// restarts the parse from the grammar's first state, so a wrong colour ends at the next break.
pub struct Painter {
    pristine: Option<Lang>,
    live: Option<Lang>,
}

impl Painter {
    pub fn new(lang: Option<Lang>) -> Self {
        Self {
            live: lang.clone(),
            pristine: lang,
        }
    }

    pub fn restart(&mut self) {
        self.live.clone_from(&self.pristine);
    }

    pub fn row(&mut self, row: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
        if row.starts_with('\u{2026}') || row.contains(ESC) {
            self.restart();
            return text_spans(row, theme, base);
        }
        let Some(lang) = self.live.as_mut() else {
            return text_spans(row, theme, base);
        };
        let spans = highlight::spans(row, lang, theme, base);
        if lang.poisoned() || row.ends_with('\u{2026}') {
            self.restart();
        }
        spans
    }
}

pub fn sniff(rows: &[String]) -> Option<Lang> {
    let first = rows
        .iter()
        .map(|row| row.trim())
        .find(|row| !row.is_empty())?;
    if ["diff --git", "--- a/", "@@ "]
        .iter()
        .any(|lead| first.starts_with(lead))
    {
        return highlight::lang_for("diff");
    }
    let opens_array = first
        .strip_prefix('[')
        .is_some_and(|rest| rest.trim_start().starts_with(['{', '"']));
    let parses = || serde_json::from_str::<serde_json::Value>(&rows.join("\n")).is_ok();
    if first.starts_with('{') || opens_array || (first.starts_with('[') && parses()) {
        return highlight::lang_for("json");
    }
    highlight::lang_for_first_line(first)
}

pub fn text_spans(row: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let path = Style::default().fg(theme.pick(Over::StatusPath, theme.accent));
    let mut out = Vec::new();
    for (text, style) in decode(row, base) {
        if style != base {
            out.push(Span::styled(text, style));
            continue;
        }
        let mut cursor = 0;
        for (start, end) in path_runs(&text) {
            if let Some(plain) = text.get(cursor..start).filter(|plain| !plain.is_empty()) {
                out.push(Span::styled(plain.to_owned(), base));
            }
            if let Some(found) = text.get(start..end) {
                out.push(Span::styled(found.to_owned(), base.patch(path)));
            }
            cursor = end;
        }
        if let Some(tail) = text.get(cursor..).filter(|tail| !tail.is_empty()) {
            out.push(Span::styled(tail.to_owned(), base));
        }
    }
    if out.is_empty() {
        out.push(Span::styled(String::new(), base));
    }
    out
}

pub fn strip_escapes(row: &str) -> String {
    decode(row, Style::default())
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// Invariant: only SGR styles; any other CSI or OSC sequence, or one cut off by a clip, is dropped.
fn decode(row: &str, base: Style) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut style = base;
    let mut text = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ESC {
            text.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                let mut params = String::new();
                let mut fin = None;
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        fin = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if fin == Some('m') {
                    if !text.is_empty() {
                        out.push((std::mem::take(&mut text), style));
                    }
                    style = sgr(&params, style, base);
                }
            }
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == BEL || (c == ESC && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {
                while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                chars.next_if(|c| ('0'..='~').contains(c));
            }
        }
    }
    if !text.is_empty() {
        out.push((text, style));
    }
    out
}

fn sgr(params: &str, mut style: Style, base: Style) -> Style {
    let codes: Vec<u16> = params
        .split([';', ':'])
        .map(|code| match code {
            "" => 0,
            code => code.parse().unwrap_or(u16::MAX),
        })
        .collect();
    let mut codes = codes.iter().copied();
    while let Some(code) = codes.next() {
        style = match code {
            0 => base,
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            30..=37 => style.fg(indexed(code - 30)),
            90..=97 => style.fg(indexed(code - 82)),
            40..=47 => style.bg(indexed(code - 40)),
            100..=107 => style.bg(indexed(code - 92)),
            39 => Style {
                fg: base.fg,
                ..style
            },
            49 => Style {
                bg: base.bg,
                ..style
            },
            38 | 48 => {
                let color = match codes.next() {
                    Some(5) => codes.next().map(indexed),
                    Some(2) => match (codes.next(), codes.next(), codes.next()) {
                        (Some(r), Some(g), Some(b)) => Some(Color::Rgb(byte(r), byte(g), byte(b))),
                        _ => None,
                    },
                    _ => None,
                };
                match (code, color) {
                    (38, Some(color)) => style.fg(color),
                    (_, Some(color)) => style.bg(color),
                    _ => style,
                }
            }
            _ => style,
        };
    }
    style
}

fn byte(code: u16) -> u8 {
    u8::try_from(code).unwrap_or(u8::MAX)
}

fn indexed(code: u16) -> Color {
    Color::Indexed(byte(code))
}

fn path_char(c: char) -> bool {
    c.is_alphanumeric() || "/._-~+@:".contains(c)
}

fn path_runs(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut url = false;
    for (at, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if c.is_whitespace() {
            url = false;
        }
        match (start, path_char(c)) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                start = None;
                let word = text.get(from..at).unwrap_or_default();
                url |= word.contains("://");
                let word = word.trim_end_matches([':', '.']);
                if !url && is_path(word) {
                    out.push((from, from.saturating_add(word.len())));
                }
            }
            _ => {}
        }
    }
    out
}

fn is_path(word: &str) -> bool {
    if word.contains("://") || word.starts_with(':') {
        return false;
    }
    let (stem, line) = match word.split_once(':') {
        Some((stem, rest)) => (
            stem,
            rest.split(':')
                .all(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty()),
        ),
        None => (word, false),
    };
    if word.contains(':') && !line {
        return false;
    }
    let last = stem.rsplit('/').next().unwrap_or(stem);
    let ext = last
        .rsplit_once('.')
        .filter(|(name, ext)| !name.is_empty() && ext.starts_with(char::is_alphabetic))
        .map(|(_, ext)| ext);
    let rooted = ["/", "./", "../", "~/"]
        .iter()
        .any(|lead| stem.starts_with(lead))
        && stem.len() > 2;
    let slashed = stem.contains('/');
    (slashed && (ext.is_some() || rooted)) || (line && ext.is_some_and(highlight::knows_extension))
}

pub(crate) fn body(
    name: &str,
    subject: &str,
    preview: &[String],
    running: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    use std::hash::{Hash, Hasher};
    type Key = (String, String, Vec<String>, bool, u64);
    thread_local! {
        static PAINTED: std::cell::RefCell<std::collections::HashMap<Key, Vec<Line<'static>>>> =
            std::cell::RefCell::default();
    }
    let mut hasher = std::hash::DefaultHasher::new();
    theme.hash(&mut hasher);
    let key: Key = (
        name.to_owned(),
        subject.to_owned(),
        preview.to_vec(),
        running,
        hasher.finish(),
    );
    if let Some(lines) = PAINTED.with_borrow(|painted| painted.get(&key).cloned()) {
        return lines;
    }
    let lines = paint(name, subject, preview, running, theme);
    PAINTED.with_borrow_mut(|painted| {
        // Incident: a visible mid-file read cost 2.3 ms of syntect a frame, and the console
        // repaints every visible card; a full table is dropped whole, not aged.
        if painted.len() >= 256 {
            painted.clear();
        }
        painted.insert(key, lines.clone());
    });
    lines
}

fn paint(
    name: &str,
    subject: &str,
    preview: &[String],
    running: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let dim = theme.dim_style();
    let text = Style::default().fg(theme.text);
    let searchy = name == "grep";
    let anchored = matches!(name, "read" | "edit");
    let streamed = running && matches!(name, "write" | "ipython");
    let lang = match name {
        "ipython" if streamed => highlight::lang_for("py"),
        _ if anchored || streamed => highlight::lang_for(subject)
            .filter(|_| !subject.is_empty())
            .or_else(|| sniff(&bodies(preview))),
        _ if searchy => None,
        _ => sniff(preview),
    };
    let mut painter = Painter::new(lang);
    let mut next: Option<u64> = None;
    let mut out = Vec::new();
    let mut file: Option<&str> = None;
    let open = |path: &str, out: &mut Vec<Line<'static>>, painter: &mut Painter| {
        out.push(Line::from(text_spans(path, theme, dim)));
        *painter = Painter::new(highlight::lang_for(path));
    };
    for raw in preview {
        let gutter = |number: &str| Span::styled(format!("{number:>4} "), dim);
        if searchy && let Some(path) = header_path(raw) {
            open(path, &mut out, &mut painter);
            next = None;
            continue;
        }
        if searchy && let Some((path, number, row)) = crate::rows::grep_row(raw) {
            if file != Some(path) {
                open(path, &mut out, &mut painter);
                file = Some(path);
            }
            painter.restart();
            let mut spans = vec![gutter(number)];
            spans.extend(painter.row(row, theme, text));
            out.push(Line::from(spans));
            continue;
        }
        let numbered =
            crate::rows::numbered(raw).or_else(|| searchy.then(|| context_row(raw)).flatten());
        if !streamed && let Some((number, row)) = numbered {
            let at = number.parse::<u64>().ok();
            if at.is_none() || at != next {
                painter.restart();
            }
            next = at.map(|at| at.saturating_add(1));
            let mut spans = vec![gutter(number)];
            spans.extend(painter.row(row, theme, text));
            out.push(Line::from(spans));
            continue;
        }
        if raw.starts_with('\u{2026}') || raw == "--" {
            painter.restart();
            next = None;
            out.push(Line::from(Span::styled(raw.clone(), dim)));
            continue;
        }
        // The hashline header repeats the path the head line already names, and the verb
        // line is the digest above it.
        if anchored {
            continue;
        }
        if searchy {
            painter.restart();
            out.push(Line::from(text_spans(raw, theme, dim)));
            continue;
        }
        out.push(Line::from(painter.row(
            raw,
            theme,
            if streamed { text } else { dim },
        )));
    }
    out
}

fn bodies(preview: &[String]) -> Vec<String> {
    preview
        .iter()
        .filter_map(|raw| crate::rows::numbered(raw).map(|(_, row)| row.to_owned()))
        .collect()
}

fn header_path(raw: &str) -> Option<&str> {
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    let (path, tag) = inner.rsplit_once('#')?;
    (!path.is_empty() && !tag.is_empty() && tag.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(path)
}

fn context_row(raw: &str) -> Option<(&str, &str)> {
    let (number, row) = raw.split_once('-')?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some((number, row))
}
