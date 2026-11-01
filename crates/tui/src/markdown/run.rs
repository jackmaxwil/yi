use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{OPTIONS, paint};
use crate::colors::Theme;

const RUN_CHIP: &str = " ▶ run ";
const SHELL_LANGS: [&str; 4] = ["bash", "sh", "shell", "zsh"];

/// An answer's rows, with a [`run_chip`] on each shell fence's header when `theme.run` says
/// the surface can press one.
pub fn render_answer(source: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    paint(source, width, theme, false, &mut None, false, theme.run)
}

/// Ends a fence's header row in the chip when the fence is a shell's.
pub(super) fn offer(head: &mut Line<'static>, info: &str, theme: &Theme) {
    if is_shell(info) {
        let chip = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::REVERSED);
        head.spans.push(Span::raw(" "));
        head.spans.push(Span::styled(RUN_CHIP, chip));
    }
}

fn is_shell(info: &str) -> bool {
    let lang = info.split_whitespace().next().unwrap_or_default();
    SHELL_LANGS
        .iter()
        .any(|shell| lang.eq_ignore_ascii_case(shell))
}

/// The code of each shell fence in `source`, in the order [`render_answer`] draws their chips.
pub fn shell_blocks(source: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut open: Option<String> = None;
    for event in Parser::new_ext(source, OPTIONS) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) if is_shell(&lang) => {
                open = Some(String::new());
            }
            Event::Text(text) => {
                if let Some(code) = &mut open {
                    code.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => blocks.extend(open.take()),
            _ => {}
        }
    }
    blocks
}

/// The columns a row's run chip covers, when its last span is one; no markdown text is
/// reversed, so a paragraph that spells the chip is not mistaken for it.
pub fn run_chip(line: &Line<'_>) -> Option<(usize, usize)> {
    let (chip, before) = line.spans.split_last()?;
    let reversed = chip.style.add_modifier.contains(Modifier::REVERSED);
    (chip.content == RUN_CHIP && reversed).then(|| {
        let start = (before.iter()).fold(0usize, |at, span| at.saturating_add(span.width()));
        (start, start.saturating_add(chip.width()))
    })
}
