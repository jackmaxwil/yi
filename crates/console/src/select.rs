//! Drag-to-copy inside one pane, prose as its markdown source, handed out over OSC 52.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use yi_tui::card::RAIL;
use yi_tui::cell::{CALLOUT_RAIL, GUTTER, USER_BAR};

use crate::app::App;
use crate::model::PaneContent;
use crate::render::{ViewState, pane_margin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
}

impl Selection {
    pub fn new(x: u16, y: u16) -> Self {
        Self {
            anchor: (x, y),
            head: (x, y),
        }
    }

    fn ends(self) -> ((u16, u16), (u16, u16)) {
        let (ax, ay) = self.anchor;
        let (hx, hy) = self.head;
        if (ay, ax) <= (hy, hx) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Paint the drag inside `clip`; a mid-clip start keeps its lead so a shared indent sheds.
pub fn paint(
    buffer: &mut Buffer,
    clip: Rect,
    selection: Selection,
    bg: Color,
) -> Vec<(u16, String)> {
    let ((x0, y0), (x1, y1)) = selection.ends();
    let last_x = clip.right().saturating_sub(1);
    let last_y = clip.bottom().saturating_sub(1);
    let mut rows = Vec::new();
    if clip.is_empty() {
        return rows;
    }
    for y in y0.max(clip.y)..=y1.min(last_y) {
        let start = if y == y0 { x0.max(clip.x) } else { clip.x };
        let end = if y == y1 { x1.min(last_x) } else { last_x };
        let mut row = " ".repeat(usize::from(start.saturating_sub(clip.x)));
        for x in start..=end {
            let Some(cell) = buffer.cell_mut(Position::new(x, y)) else {
                continue;
            };
            cell.set_bg(bg);
            row.push_str(cell.symbol());
        }
        rows.push((y, row.trim_end().to_owned()));
    }
    rows
}

/// The words under a drag, not the frame: Yi's own chrome (the card rail, the callout rail,
/// the user bar, the assistant gutter) drops off each row, then the rows shed a shared indent.
pub fn words(rows: &[String]) -> String {
    let stripped: Vec<String> = rows.iter().map(|row| strip_chrome(row)).collect();
    let indent = stripped
        .iter()
        .filter(|row| !row.trim().is_empty())
        .map(|row| row.len().saturating_sub(row.trim_start().len()))
        .min()
        .unwrap_or(0);
    let text = stripped
        .iter()
        .enumerate()
        .map(|(n, row)| {
            let row = row.get(indent..).unwrap_or_default();
            if n == 0 { row.trim_start() } else { row }
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.trim_matches('\n').to_owned()
}

const MARKUP: &str = "*_`~#>|\\-+[]()‣◦▪•│┃▌─━—┌┐└┘├┤┬┴┼";
const WRAPS: &str = "*_`~";

fn markup(ch: char) -> bool {
    ch.is_whitespace() || MARKUP.contains(ch)
}

/// The source behind a drag's words, grown over markup; `None` when the render differs.
pub fn source_span(source: &str, shown: &str) -> Option<String> {
    let wanted: String = shown.chars().filter(|&ch| !markup(ch)).collect();
    if wanted.is_empty() {
        return None;
    }
    let (mut key, mut at) = (String::new(), Vec::new());
    for (offset, ch) in source.char_indices().filter(|&(_, ch)| !markup(ch)) {
        key.push(ch);
        at.push((offset, offset.saturating_add(ch.len_utf8())));
    }
    let first = key.get(..key.find(&wanted)?)?.chars().count();
    let last = first
        .saturating_add(wanted.chars().count())
        .checked_sub(1)?;
    let (mut start, _) = *at.get(first)?;
    let (_, mut end) = *at.get(last)?;

    let before = source.get(..start)?;
    let opened = before.trim_end_matches(|ch| WRAPS.contains(ch));
    if !opened
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
    {
        start = opened.len();
    }
    let before = source.get(..start)?;
    let line_start = before.rfind('\n').map_or(0, |n| n.saturating_add(1));
    if source.get(line_start..start)?.chars().all(markup) {
        start = line_start;
    }
    let closed = source
        .get(end..)?
        .trim_start_matches(|ch| WRAPS.contains(ch));
    if !closed.chars().next().is_some_and(char::is_alphanumeric) {
        end = source.len().saturating_sub(closed.len());
    }
    let line_end = source
        .get(end..)?
        .find('\n')
        .map_or(source.len(), |n| end.saturating_add(n));
    if source.get(end..line_end)?.chars().all(markup) {
        end = line_end;
    }
    let fences = |text: &str| {
        text.lines()
            .filter(|line| {
                line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~")
            })
            .count()
    };
    if fences(source.get(start..end)?) % 2 == 1 {
        let mut offset = end;
        for (index, line) in source.get(end..)?.split_inclusive('\n').enumerate() {
            offset = offset.saturating_add(line.len());
            if index > 0 && fences(line) == 1 {
                end = offset;
                break;
            }
        }
    }
    Some(source.get(start..end)?.trim_matches('\n').to_owned())
}

/// Rows grouped by cell: a cell with a source copies it, others their words, one blank between.
pub fn copy<'s>(
    rows: &[(u16, String)],
    owner: impl Fn(u16) -> (Option<usize>, Option<&'s str>),
) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let mut group: Vec<String> = Vec::new();
    let mut current = None;
    let mut flush = |group: &mut Vec<String>, source: Option<&str>| {
        let shown = words(group);
        let text = source
            .and_then(|source| source_span(source, &shown))
            .unwrap_or(shown);
        if !text.trim().is_empty() {
            blocks.push(text);
        }
        group.clear();
    };
    for (y, row) in rows {
        let (cell, source) = owner(*y);
        if current.is_some_and(|(held, _)| held != cell) {
            flush(&mut group, current.and_then(|(_, source)| source));
        }
        current = Some((cell, source));
        group.push(row.clone());
    }
    flush(&mut group, current.and_then(|(_, source)| source));
    blocks.join("\n\n")
}

fn strip_chrome(row: &str) -> String {
    let body = row.trim_start();
    let lead = " ".repeat(row.len().saturating_sub(body.len()));
    // The card rail and the gutter carry their own space, and a card body's inset is its
    // shape; the user bar and the callout rail pad with spaces that are only chrome.
    for glyph in [RAIL, GUTTER] {
        if let Some(rest) = body.strip_prefix(glyph) {
            return format!("{lead}{rest}");
        }
        if body == glyph.trim_end() {
            return String::new();
        }
    }
    for glyph in [CALLOUT_RAIL, USER_BAR] {
        if let Some(rest) = body.strip_prefix(glyph) {
            return format!("{lead}{}", rest.trim_start());
        }
    }
    row.to_owned()
}

pub fn selected(
    app: &App,
    view: &ViewState,
    buffer: &mut Buffer,
    selection: Selection,
    bg: Color,
) -> String {
    let (x, y) = selection.anchor;
    let Some(pane) = view
        .panes
        .iter()
        .find(|pane| pane.rect.contains(Position::new(x, y)))
    else {
        return String::new();
    };
    let inner = pane.rect.inner(pane_margin(view.framed));
    match app.state.panes.get(&pane.id).map(|pane| &pane.content) {
        Some(PaneContent::Session {
            chat: Some(chat), ..
        }) => {
            let rows: Vec<u16> = (inner.top()..inner.bottom())
                .filter(|&y| chat.app.pane_row(y).is_some())
                .collect();
            let (Some(&top), Some(&bottom)) = (rows.first(), rows.last()) else {
                return String::new();
            };
            let clip = Rect {
                y: top,
                height: bottom.saturating_sub(top).saturating_add(1),
                ..inner
            };
            let rows = paint(buffer, clip, selection, bg);
            copy(&rows, |y| match chat.app.pane_row(y).flatten() {
                Some((cell, source)) => (Some(cell), source),
                None => (None, None),
            })
        }
        Some(PaneContent::Markdown { source, .. }) => {
            copy(&paint(buffer, inner, selection, bg), |_| {
                (None, Some(source))
            })
        }
        _ => copy(&paint(buffer, inner, selection, bg), |_| (None, None)),
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let byte = |n: usize| u32::from(chunk.get(n).copied().unwrap_or(0));
        let packed = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for slot in 0..4 {
            let symbol = if slot <= chunk.len() {
                let index = usize::try_from((packed >> (18 - 6 * slot)) & 63).unwrap_or(0);
                ALPHABET.get(index).copied().unwrap_or(b'=')
            } else {
                b'='
            };
            out.push(char::from(symbol));
        }
    }
    out
}

pub fn osc52(text: &str) -> String {
    format!("\u{1b}]52;c;{}\u{7}", base64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_every_tail_length() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"abcdef"), "YWJjZGVm");
        assert_eq!(base64(&[0xff, 0xfe]), "//4=");
        assert_eq!(osc52("hi"), "\u{1b}]52;c;aGk=\u{7}");
    }

    #[test]
    fn a_drag_reads_the_cells_under_it_in_reading_order() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 3));
        buffer.set_string(0, 0, "hello", ratatui::style::Style::default());
        buffer.set_string(0, 1, "world", ratatui::style::Style::default());

        let rows = paint(
            &mut buffer,
            Rect::new(0, 0, 10, 3),
            Selection {
                anchor: (1, 0),
                head: (3, 0),
            },
            Color::DarkGray,
        );
        assert_eq!(copy(&rows, |_| (None, None)), "ell");

        let rows = paint(
            &mut buffer,
            Rect::new(0, 0, 10, 3),
            Selection {
                anchor: (2, 1),
                head: (2, 0),
            },
            Color::DarkGray,
        );
        assert_eq!(copy(&rows, |_| (None, None)), "llo\nwor");
        assert_eq!(
            buffer.cell(Position::new(2, 0)).map(|cell| cell.bg),
            Some(Color::DarkGray),
            "the drag is painted where it read"
        );
    }
}
