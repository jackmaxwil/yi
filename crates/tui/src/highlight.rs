use std::sync::OnceLock;

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::colors::Theme;

/// A line longer than this is generated, not written, and colouring it costs
/// more than reading it is worth.
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
    Variable,
}

#[derive(Clone)]
pub struct Lang {
    state: ParseState,
    stack: ScopeStack,
    /// A parse error leaves the incremental state no longer describing the text, so colour
    /// stops for the rest of the block rather than painting the wrong words as keywords.
    poisoned: bool,
}

/// Decompressed on first use, never on the startup path §13.6 holds to 5 ms.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Scope prefixes, most specific first. `storage.type.numeric` is a literal's suffix and
/// belongs to its number; every other `storage` is a keyword, as Rust scopes `let` and `usize`.
fn scope_table() -> &'static [(Scope, Token)] {
    static TABLE: OnceLock<Vec<(Scope, Token)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        [
            ("comment", Token::Comment),
            ("string", Token::Str),
            ("constant.character.escape", Token::Str),
            ("constant.numeric", Token::Number),
            ("storage.type.numeric", Token::Number),
            ("constant", Token::Keyword),
            ("keyword", Token::Keyword),
            ("storage", Token::Keyword),
            ("entity.name.function", Token::Function),
            ("support.function", Token::Function),
            ("variable.function", Token::Function),
            ("variable", Token::Variable),
            ("entity.name", Token::Type),
            ("support.type", Token::Type),
            ("support.class", Token::Type),
        ]
        .iter()
        .filter_map(|(text, token)| Scope::new(text).ok().map(|scope| (scope, *token)))
        .collect()
    })
}

fn token_for(stack: &ScopeStack) -> Token {
    for scope in stack.as_slice().iter().rev() {
        if let Some((_, token)) = scope_table()
            .iter()
            .find(|(prefix, _)| prefix.is_prefix_of(*scope))
        {
            return *token;
        }
    }
    Token::Plain
}

/// Fence names models write that no bundled grammar answers to. The default set has no
/// TypeScript at all; JavaScript is the closest that colours it instead of leaving it plain.
fn alias(name: &str) -> &str {
    match name {
        "shell" => "sh",
        "python3" | "ipython" => "py",
        "ts" | "tsx" | "jsx" | "typescript" => "js",
        "jsonc" => "json",
        other => other,
    }
}

pub fn lang_for(name: &str) -> Option<Lang> {
    let set = syntaxes();
    let name = name
        .rsplit(['.', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    let name = alias(&name);
    let syntax = set
        .find_syntax_by_token(name)
        .or_else(|| set.find_syntax_by_extension(name))?;
    Some(Lang {
        state: ParseState::new(syntax),
        stack: ScopeStack::new(),
        poisoned: false,
    })
}

impl Theme {
    /// Foreground and bold only: a background would fight the diff tint it
    /// renders inside, and terminals render italic and underline least alike.
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
            Token::Variable => Style::default().fg(self.accent),
        }
    }
}

/// Advances the parse, so the next line resumes where this one left off. The cap drops a
/// generated line's runs, never its parse, or a later line resumes from a stale state.
pub fn tokens(line: &str, lang: &mut Lang) -> Vec<(usize, usize, Token)> {
    if lang.poisoned {
        return Vec::new();
    }
    let owned = format!("{line}\n");
    let Ok(ops) = lang.state.parse_line(&owned, syntaxes()) else {
        lang.poisoned = true;
        return Vec::new();
    };
    let mut out: Vec<(usize, usize, Token)> = Vec::new();
    let mut cursor = 0_usize;
    for (offset, op) in ops {
        let end = offset.min(line.len());
        if end > cursor {
            let token = token_for(&lang.stack);
            if token != Token::Plain {
                out.push((cursor, end, token));
            }
        }
        cursor = cursor.max(end);
        if lang.stack.apply(&op).is_err() {
            lang.poisoned = true;
            return Vec::new();
        }
    }
    if cursor < line.len() {
        let token = token_for(&lang.stack);
        if token != Token::Plain {
            out.push((cursor, line.len(), token));
        }
    }
    if line.len() > LINE_CAP {
        return Vec::new();
    }
    merge(out)
}

/// Adjacent runs of one kind are one span: the grammar splits an identifier
/// from its scope operator, and the reader wants the word.
fn merge(runs: Vec<(usize, usize, Token)>) -> Vec<(usize, usize, Token)> {
    let mut out: Vec<(usize, usize, Token)> = Vec::with_capacity(runs.len());
    for (start, end, token) in runs {
        match out.last_mut() {
            Some(last) if last.2 == token && last.1 == start => last.1 = end,
            _ => out.push((start, end, token)),
        }
    }
    out
}

/// One line as styled spans. `base` carries what the caller already decided about the row —
/// the diff tint, a dim body — and each token adds only its foreground.
pub fn spans(line: &str, lang: &mut Lang, theme: &Theme, base: Style) -> Vec<Span<'static>> {
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
