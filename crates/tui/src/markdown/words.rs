//! Which words in a prose run look like a path, a link or code, and the hue each one wears.
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::colors::{Over, Theme};

pub(super) fn code_hue(theme: &Theme, code: &str) -> Color {
    if let Some(color) = theme.from_file(Over::MdCode) {
        return color;
    }
    let path_like = (code.contains('/') && !code.contains(' ')) || is_path(code);
    if path_like {
        crate::card::tool_hue(theme, "read")
    } else {
        theme.orange
    }
}

fn is_path(word: &str) -> bool {
    if word.contains(char::is_whitespace) {
        return false;
    }
    let rooted = ["./", "../", "~/"]
        .iter()
        .any(|root| word.starts_with(root))
        || (word.starts_with('/') && word.get(1..).is_some_and(|rest| rest.contains('/')));
    let mut bare = word;
    for _ in 0..2 {
        if let Some((head, number)) = bare.rsplit_once(':')
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
        {
            bare = head;
        }
    }
    let name = bare.rsplit('/').next().unwrap_or(bare);
    let extension = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        let product = ext == "js" && RUNTIMES.contains(&stem);
        (stem.contains(char::is_alphabetic) || ext.len() > 1)
            && !product
            && stem
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && KNOWN_EXTENSIONS.contains(&ext)
    });
    rooted || extension
}

const RUNTIMES: [&str; 6] = ["Node", "Next", "Nuxt", "React", "Vue", "Three"];

const KNOWN_EXTENSIONS: [&str; 44] = [
    "c", "cc", "cfg", "cpp", "css", "csv", "go", "h", "hpp", "html", "ini", "java", "jpg", "js",
    "json", "jsonl", "jsx", "kt", "lock", "log", "lua", "md", "mjs", "patch", "pdf", "php", "png",
    "py", "pyi", "rb", "rs", "scss", "sh", "sql", "svg", "swift", "toml", "ts", "tsx", "txt",
    "xml", "yaml", "yml", "zsh",
];

pub(crate) fn marked_words(text: &str, style: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let mut core = word
            .trim_start_matches(['(', '[', '"', '\''])
            .trim_end_matches(['.', ',', ';', ':', ']', '!', '?', '"', '\'']);
        while core.ends_with(')') && core.matches(')').count() > core.matches('(').count() {
            core = core
                .strip_suffix(')')
                .unwrap_or(core)
                .trim_end_matches(['.', ',', ';', ':']);
        }
        let hue = if core.starts_with("https://") || core.starts_with("http://") {
            Some(theme.blue5)
        } else if is_path(core) {
            Some(crate::card::tool_hue(theme, "read"))
        } else {
            None
        };
        let (Some(hue), Some(at)) = (hue, word.find(core).filter(|_| !core.is_empty())) else {
            plain.push_str(piece);
            continue;
        };
        plain.push_str(word.get(..at).unwrap_or_default());
        if !plain.is_empty() {
            out.push(Span::styled(std::mem::take(&mut plain), style));
        }
        out.push(Span::styled(core.to_owned(), style.fg(hue)));
        plain.push_str(
            piece
                .get(at.saturating_add(core.len())..)
                .unwrap_or_default(),
        );
    }
    if !plain.is_empty() || out.is_empty() {
        out.push(Span::styled(plain, style));
    }
    out
}
