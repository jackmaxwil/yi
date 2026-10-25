//! The editor pane's rows, cursor cell and click mapping, by drawn column rather than by char.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use yi_tui::colors::Theme;
use yi_tui::markdown::{cell_width, expand_tabs_at as expand};

use crate::model::Editor;

fn drawn_col(line: &str, index: usize) -> usize {
    line.chars()
        .take(index)
        .fold(0, |at, c| at.saturating_add(cell_width(c, at)))
}

/// The char drawn under column `col` of `line`, or the line's end past its last char.
/// ponytail: by char width, so a VS16 or ZWJ emoji before `col` shifts it; walk graphemes then.
pub(crate) fn char_at(line: &str, col: usize) -> usize {
    let mut at: usize = 0;
    for (index, c) in line.chars().enumerate() {
        at = at.saturating_add(cell_width(c, at));
        if col < at {
            return index;
        }
    }
    line.chars().count()
}

pub(crate) fn cursor_cell(editor: &Editor, inner: Rect) -> Option<(u16, u16)> {
    let (row, col) = editor.text.cursor();
    let line = editor.text.lines().get(row)?;
    // Incident: an emoji with VS16 draws two cells where its chars sum to one; take the drawn
    // prefix's width, as the buffer does.
    let prefix: String = line.chars().take(col).collect();
    let x = inner
        .x
        .checked_add(u16::try_from(editor.gutter()).ok()?)?
        .checked_add(u16::try_from(expand(&prefix, 0).width()).ok()?)?;
    let y = inner
        .y
        .checked_add(u16::try_from(row.checked_sub(editor.scroll_top)?).ok()?)?;
    (x < inner.x.saturating_add(inner.width) && y < inner.y.saturating_add(inner.height))
        .then_some((x, y))
}

pub(crate) fn view(
    editor: &mut Editor,
    inner: Rect,
    theme: &Theme,
) -> (String, Vec<Line<'static>>, Option<(usize, usize)>) {
    let height = usize::from(inner.height).max(1);
    let total = editor.text.lines().len();
    let cursor = editor.text.cursor();
    if cursor != editor.last_cursor {
        editor.last_cursor = cursor;
        if cursor.0 < editor.scroll_top {
            editor.scroll_top = cursor.0;
        } else if cursor.0 >= editor.scroll_top.saturating_add(height) {
            editor.scroll_top = cursor.0.saturating_add(1).saturating_sub(height);
        }
    }
    editor.scroll_top = editor.scroll_top.min(total.saturating_sub(1));
    let gutter = editor.gutter();
    let mut lang = editor.primed_lang();
    let selection = editor.text.selection_range();
    let base = Style::default().fg(theme.text);
    let mut out = Vec::new();
    for (index, line) in editor
        .text
        .lines()
        .iter()
        .enumerate()
        .skip(editor.scroll_top)
        .take(height)
    {
        let number = format!(
            "{:>width$} ",
            index.saturating_add(1),
            width = gutter.saturating_sub(1)
        );
        let mut spans = vec![Span::styled(number, theme.dim_style())];
        let selected = selection.and_then(|(start, end)| {
            let from = if index == start.0 { start.1 } else { 0 };
            let to = if index == end.0 {
                end.1
            } else {
                line.chars().count()
            };
            (start.0 <= index && index <= end.0).then_some((from, to))
        });
        match (selected, lang.as_mut()) {
            (Some((from, to)), _) => {
                let head: String = line.chars().take(from).collect();
                let mid: String = line
                    .chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect();
                let tail: String = line.chars().skip(to).collect();
                let head = expand(&head, 0);
                let mid = expand(&mid, drawn_col(line, from));
                let tail = expand(&tail, drawn_col(line, to));
                spans.push(Span::styled(head, base));
                spans.push(Span::styled(mid, base.add_modifier(Modifier::REVERSED)));
                spans.push(Span::styled(tail, base));
            }
            (None, Some(lang)) => {
                let drawn = expand(line, 0);
                spans.extend(yi_tui::highlight::spans(&drawn, lang, theme, base));
            }
            (None, None) => spans.push(Span::styled(expand(line, 0), base)),
        }
        out.push(Line::from(spans));
    }
    if editor.stale {
        out.truncate(height.saturating_sub(1));
        out.push(Line::styled(
            "file changed on disk · r reload · k keep",
            Style::default().fg(theme.warning),
        ));
    }
    let mark = if editor.dirty { " ●" } else { "" };
    let extent = (total > height).then_some((total, editor.scroll_top));
    (
        format!("✎ {}{mark}", crate::render::short_path(&editor.path)),
        out,
        extent,
    )
}
