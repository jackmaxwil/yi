use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::colors::Theme;

/// A line longer than this is generated, not written, and colouring it costs
/// more than reading it is worth (codex `highlight.rs`, same intent).
const LINE_CAP: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Comment,
    Str,
    Number,
    Keyword,
    Type,
    Function,
}

pub struct Lang {
    line_comment: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    keywords: &'static [&'static str],
    /// In shell `#` comments only at a word boundary (`x#y` is literal).
    comment_at_word_start: bool,
    /// `'` opens a char literal only when it closes nearby — a lifetime
    /// (`'a`) never closes and must not open a string.
    char_literal: bool,
}

const RUST: Lang = Lang {
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"'],
    comment_at_word_start: false,
    char_literal: true,
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
};

const PYTHON: Lang = Lang {
    line_comment: &["#"],
    block: None,
    quotes: &['"', '\''],
    comment_at_word_start: false,
    char_literal: false,
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
        "try", "while", "with", "yield",
    ],
};

const SHELL: Lang = Lang {
    line_comment: &["#"],
    block: None,
    quotes: &['"', '\''],
    comment_at_word_start: true,
    char_literal: false,
    keywords: &[
        "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
        "in", "local", "return", "set", "then", "until", "while",
    ],
};

const JSON: Lang = Lang {
    line_comment: &[],
    block: None,
    quotes: &['"'],
    comment_at_word_start: false,
    char_literal: false,
    keywords: &["true", "false", "null"],
};

const TS: Lang = Lang {
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"', '\'', '`'],
    comment_at_word_start: false,
    char_literal: false,
    keywords: &[
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "return",
        "static",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "while",
        "yield",
    ],
};

/// Yi's own surfaces name five languages: Rust and Python source, shell
/// commands, JSON payloads and TypeScript in the reference codebases. An
/// unknown name renders plain, which is what it did before.
pub fn lang_for(name: &str) -> Option<&'static Lang> {
    let name = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    match name.as_str() {
        "rs" | "rust" => Some(&RUST),
        "py" | "python" | "python3" | "ipython" => Some(&PYTHON),
        "sh" | "bash" | "zsh" | "shell" => Some(&SHELL),
        "json" => Some(&JSON),
        "ts" | "tsx" | "js" | "jsx" | "typescript" | "javascript" => Some(&TS),
        _ => None,
    }
}

impl Theme {
    /// codex `highlight.rs`: foreground and bold only. A background
    /// would fight the diff tint it renders inside, and italic and underline are
    /// the two attributes terminals render least consistently.
    pub fn syntax_style(&self, token: Token) -> Style {
        match token {
            Token::Plain => Style::default().fg(self.text),
            Token::Comment => self.dim_style(),
            Token::Str => Style::default().fg(self.success),
            Token::Number => Style::default().fg(self.warning),
            Token::Keyword => Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
            Token::Type => Style::default().fg(self.muted).add_modifier(Modifier::BOLD),
            Token::Function => Style::default().fg(self.warning),
        }
    }
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// A single line's tokens as `(byte range, kind)`. Per-line scanning: the
/// audited limits (2026-08-29) are multi-line `/* */`, triple-quoted, raw and
/// template strings colouring only their opening line; regex literals plain.
pub fn tokens(line: &str, lang: &Lang) -> Vec<(usize, usize, Token)> {
    if line.len() > LINE_CAP {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        let rest = line.get(index..).unwrap_or_default();
        let at_word_start = index == 0
            || bytes
                .get(index.wrapping_sub(1))
                .is_some_and(|byte| byte.is_ascii_whitespace());
        if lang.line_comment.iter().any(|c| rest.starts_with(c))
            && (!lang.comment_at_word_start || at_word_start)
        {
            out.push((index, line.len(), Token::Comment));
            break;
        }
        if let Some((open, close)) = lang.block
            && rest.starts_with(open)
        {
            // A block comment closing on the same line ends there; the code
            // after it scans normally.
            match rest.get(open.len()..).and_then(|tail| tail.find(close)) {
                Some(offset) => {
                    let end = index + open.len() + offset + close.len();
                    out.push((index, end, Token::Comment));
                    index = end;
                    continue;
                }
                None => {
                    out.push((index, line.len(), Token::Comment));
                    break;
                }
            }
        }
        let ch = rest.chars().next().unwrap_or(' ');
        if ch == '\'' && lang.char_literal {
            // A char literal holds one char or an escape (`'x'`, `'\n'`,
            // `'\u{1F600}'`). A lifetime (`'a`) fails that shape — including
            // the `'a, '` span two lifetimes would fake — and stays plain.
            let literal = string_end(rest, '\'').filter(|end| {
                rest.get(1..end.saturating_sub(1))
                    .is_some_and(|body| body.starts_with('\\') || body.chars().count() == 1)
            });
            match literal {
                Some(end) => {
                    out.push((index, index.saturating_add(end), Token::Str));
                    index = index.saturating_add(end);
                }
                None => index = index.saturating_add(1),
            }
            continue;
        }
        if lang.quotes.contains(&ch) {
            let end = string_end(rest, ch).unwrap_or(rest.len());
            out.push((index, index.saturating_add(end), Token::Str));
            index = index.saturating_add(end);
            continue;
        }
        if ch.is_ascii_digit() {
            let len = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
                .map(char::len_utf8)
                .sum::<usize>();
            out.push((index, index.saturating_add(len), Token::Number));
            index = index.saturating_add(len);
            continue;
        }
        if is_ident(ch) {
            let len = rest
                .chars()
                .take_while(|c| is_ident(*c))
                .map(char::len_utf8)
                .sum::<usize>();
            let word = rest.get(..len).unwrap_or_default();
            let after = rest.get(len..).unwrap_or_default();
            let kind = if lang.keywords.contains(&word) {
                Token::Keyword
            } else if after.starts_with('(') {
                Token::Function
            // An uppercase initial with a lowercase in it is a type name;
            // all-caps is a constant, which is not one.
            } else if word.starts_with(char::is_uppercase) && word.contains(char::is_lowercase) {
                Token::Type
            } else {
                Token::Plain
            };
            if kind != Token::Plain {
                out.push((index, index.saturating_add(len), kind));
            }
            index = index.saturating_add(len);
            continue;
        }
        index = index.saturating_add(ch.len_utf8());
    }
    out
}

/// The byte offset just past the closing quote, honouring a backslash escape.
fn string_end(rest: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (offset, ch) in rest.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some(offset.saturating_add(ch.len_utf8()));
        }
    }
    None
}

/// One line as styled spans. `base` carries whatever the caller already decided
/// about the row — the diff tint, a dim body — and each token adds only its
/// foreground, so a highlighted row keeps its background.
pub fn spans(line: &str, lang: &Lang, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut cursor = 0_usize;
    for (start, end, token) in tokens(line, lang) {
        if start > cursor
            && let Some(plain) = line.get(cursor..start)
        {
            out.push(Span::styled(plain.to_owned(), base));
        }
        if let Some(text) = line.get(start..end) {
            let fg = theme.syntax_style(token);
            let mut style = base;
            if let Some(color) = fg.fg {
                style = style.fg(color);
            }
            out.push(Span::styled(
                text.to_owned(),
                style.patch(Style::default().add_modifier(fg.add_modifier)),
            ));
        }
        cursor = end;
    }
    if let Some(tail) = line.get(cursor..)
        && !tail.is_empty()
    {
        out.push(Span::styled(tail.to_owned(), base));
    }
    if out.is_empty() {
        out.push(Span::styled(line.to_owned(), base));
    }
    out
}
