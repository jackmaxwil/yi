use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::cell::{TranscriptMode, count_label, elapsed_label};
use crate::colors::Theme;
use crate::markdown;
use crate::wrap::wrap_line;

const THOUGHT_INDENT: &str = "  ";
pub(crate) const FOLDED: &str = " · ctrl+o";

/// A thought's one-line face, `thought · 24s · 8.3k chars · ctrl+o`, counted, never summarised;
/// `cut` names what it hides from ctrl+o's verbose view, `took` the live ms replay lacks.
pub fn head(
    word: &str,
    markdown: &str,
    took: Option<u64>,
    cut: &str,
    theme: &Theme,
) -> Line<'static> {
    let chars = markdown.chars().count();
    let size = match chars {
        0..1000 => count_label(chars, "char"),
        _ => format!("{}.{}k chars", chars / 1000, chars % 1000 / 100),
    };
    let when = took.map_or_else(String::new, |ms| format!(" · {}", elapsed_label(ms)));
    let style = theme.dim_style().add_modifier(Modifier::ITALIC);
    Line::from(Span::styled(format!("  {word}{when} · {size}{cut}"), style))
}

/// Reasoning prose, dim and italic, with nothing over it: the style is the label. `header`
/// is false past the first slice, so a thought streamed in paragraphs opens one blank, not many.
pub fn lines(
    markdown: &str,
    took: Option<u64>,
    width: usize,
    theme: &Theme,
    mode: TranscriptMode,
    header: bool,
) -> Vec<Line<'static>> {
    let style = theme.dim_style().add_modifier(Modifier::ITALIC);
    if mode != TranscriptMode::Verbose {
        return vec![head("thought", markdown, took, FOLDED, theme)];
    }
    let mut out = Vec::new();
    if header {
        out.push(Line::default());
    }
    // Rendered two columns narrow, matching the indent below: a full-width row wrapped again.
    // Plain, since the restyle drops syntax colour; untrimmed, since a row's lead is its nesting.
    for line in markdown::render_plain(markdown, width.saturating_sub(THOUGHT_INDENT.len()), theme)
    {
        let text: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        out.extend(wrap_line(
            &Line::from(Span::styled(format!("{THOUGHT_INDENT}{text}"), style)),
            width,
            THOUGHT_INDENT,
        ));
    }
    out
}
