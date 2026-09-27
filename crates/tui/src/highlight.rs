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
    Deleted,
}

#[derive(Clone)]
pub struct Lang {
    state: ParseState,
    stack: ScopeStack,
    /// A parse error leaves the incremental state no longer describing the text, so colour
    /// stops for the rest of the block rather than painting the wrong words as keywords.
    poisoned: bool,
    infer: bool,
}

const INFERRED: [&str; 15] = [
    "Rust",
    "Go",
    "Java",
    "Kotlin",
    "Swift",
    "JavaScript",
    "TypeScript",
    "TypeScriptReact",
    "C",
    "C++",
    "C#",
    "Python",
    "Scala",
    "Groovy",
    "Objective-C",
];

/// bat's set, since syntect's has no TOML or TypeScript; decompressed on first use, off startup.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let _span = yi_types::trace::span("tui.load_syntaxes");
        two_face::syntax::extra_newlines()
    })
}

/// Loads common grammars off-thread: syntect's first line of one cost 55-370 ms in a frame.
pub fn prewarm() {
    let _ = std::thread::Builder::new()
        .name("yi-highlight-warm".to_owned())
        .spawn(|| {
            let _span = yi_types::trace::span("tui.highlight_prewarm");
            for (name, sample) in [
                (
                    "rs",
                    "//! d\n#[derive(Debug)]\npub struct S<'a> { x: &'a str }\n\
                     impl<T: Clone> Tr for S<T> where T: Send {\n    /// d\n\
                     pub async fn f(&mut self) -> Result<u8, E> {\n\
                     let v = vec![1_u8, 0x2]; /* b */ // c\n\
                     match v.get(0) { Some(n) if *n > 1 => Ok(*n), _ => Err(\"e\".into()) }\n\
                     let c = |a: u8| -> u8 { a as u8 };\n    }\n}\n\
                     use std::io::{self, Write};\nconst X: &str = r#\"raw\"#;\n\
                     macro_rules! m { ($e:expr) => { $e }; }",
                ),
                ("py", "def f(x):\n    return f\"{x}\"  # c"),
                ("ts", "const x: number = f(`a${b}`); // c"),
                ("js", "export const x = () => ({ a: 1 });"),
                ("sh", "for f in *; do echo \"$f\"; done # c"),
                ("json", "{\"a\": [1, true, null]}"),
                ("toml", "[a]\nb = \"c\" # d"),
                ("yaml", "a:\n  - b: 'c' # d"),
                (
                    "md",
                    "# h\n- `x` **y** _z_ [l](u)\n> q\n\n| a | b |\n|---|---|\n\
                     1. i\n```rust\nlet x = 1;\n```",
                ),
                ("diff", "@@ -1 +1 @@\n-a\n+b"),
            ] {
                if let Some(mut lang) = lang_for(name) {
                    for line in sample.lines() {
                        let _ = tokens(line, &mut lang);
                    }
                }
            }
        });
}

/// Scope prefixes, most specific first. `storage.type.numeric` is a literal's suffix and
/// belongs to its number; every other `storage` is a keyword, as Rust scopes `let` and `usize`.
fn scope_table() -> &'static [(Scope, Token)] {
    static TABLE: OnceLock<Vec<(Scope, Token)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        [
            ("comment", Token::Comment),
            ("markup.inserted", Token::Str),
            ("markup.deleted", Token::Deleted),
            ("meta.diff.range", Token::Function),
            ("meta.diff.header", Token::Keyword),
            ("markup.heading", Token::Type),
            ("markup.raw", Token::Str),
            ("string", Token::Str),
            ("constant.character.escape", Token::Str),
            ("constant.numeric", Token::Number),
            ("storage.type.numeric", Token::Number),
            ("constant", Token::Keyword),
            ("keyword", Token::Keyword),
            ("storage", Token::Keyword),
            ("entity.name.function", Token::Function),
            ("support.function", Token::Function),
            ("support.macro", Token::Function),
            ("variable.function", Token::Function),
            ("variable.annotation", Token::Function),
            ("meta.annotation", Token::Function),
            ("entity.other.attribute-name", Token::Function),
            // JS and TS scope every plain identifier a variable: a wall of magenta.
            ("variable.other.readwrite.js", Token::Plain),
            ("variable.other.readwrite.ts", Token::Plain),
            ("variable.other.readwrite.tsx", Token::Plain),
            ("variable.other.object", Token::Plain),
            ("variable.other.property", Token::Plain),
            ("variable.other.constant.ts", Token::Plain),
            ("variable.other.constant.tsx", Token::Plain),
            ("variable", Token::Variable),
            ("entity.other.inherited-class", Token::Type),
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

/// Fence names models write that no grammar answers to by name or extension.
fn alias(name: &str) -> &str {
    match name {
        "shell" => "sh",
        "python3" | "ipython" => "py",
        "jsx" | "mjs" | "cjs" => "js",
        "mts" | "cts" => "ts",
        "jsonc" | "jsonl" | "json5" => "json",
        "conf" | "editorconfig" => "ini",
        "golang" => "go",
        "csharp" => "cs",
        "containerfile" => "dockerfile",
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
        infer: INFERRED.contains(&syntax.name.as_str()),
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
            Token::Number => Style::default().fg(self.orange),
            Token::Keyword => Style::default()
                .fg(self.magenta)
                .add_modifier(Modifier::BOLD),
            Token::Type => Style::default().fg(self.teal).add_modifier(Modifier::BOLD),
            Token::Function => Style::default().fg(self.accent),
            Token::Variable => Style::default().fg(self.magenta),
            Token::Deleted => Style::default().fg(self.error),
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
    let parsing = yi_types::trace::span("tui.highlight_line");
    let parsed = lang.state.parse_line(&owned, syntaxes());
    drop(parsing);
    let Ok(ops) = parsed else {
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
    if lang.infer {
        out = with_inferred(line, out);
    }
    merge(out)
}

fn with_inferred(line: &str, runs: Vec<(usize, usize, Token)>) -> Vec<(usize, usize, Token)> {
    let mut out = Vec::with_capacity(runs.len());
    let mut cursor = 0_usize;
    for run in runs
        .into_iter()
        .chain(std::iter::once((line.len(), line.len(), Token::Plain)))
    {
        let bytes = line.as_bytes();
        let mut at = cursor;
        while at < run.0 {
            let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || !b.is_ascii();
            let starts = bytes
                .get(at)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || !b.is_ascii())
                && (at == 0 || !bytes.get(at - 1).copied().is_some_and(word));
            if !starts {
                at += 1;
                continue;
            }
            let end = (at..run.0)
                .find(|index| !bytes.get(*index).copied().is_some_and(word))
                .unwrap_or(run.0);
            let text = line.get(at..end).unwrap_or_default();
            let token = if bytes.get(end) == Some(&b'(') {
                Token::Function
            } else if text.starts_with(|c: char| c.is_ascii_uppercase())
                && text.contains(|c: char| c.is_ascii_lowercase())
            {
                Token::Type
            } else {
                Token::Plain
            };
            if token != Token::Plain {
                out.push((at, end, token));
            }
            at = end;
        }
        if run.0 < run.1 {
            out.push(run);
        }
        cursor = cursor.max(run.1);
    }
    out
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
