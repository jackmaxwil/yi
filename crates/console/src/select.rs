//! Drag-to-copy over the drawn frame: the cells under the drag are the text, so every pane
//! kind copies what is on screen, and the release hands it to the terminal over OSC 52.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

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

/// Paint the drag and return the text under it; rows between the two ends run the full
/// width, the way a terminal's own selection does.
pub fn paint(buffer: &mut Buffer, area: Rect, selection: Selection, bg: Color) -> String {
    let ((x0, y0), (x1, y1)) = selection.ends();
    let last_x = area.right().saturating_sub(1);
    let last_y = area.bottom().saturating_sub(1);
    let mut text = String::new();
    for y in y0.max(area.y)..=y1.min(last_y) {
        let start = if y == y0 { x0.max(area.x) } else { area.x };
        let end = if y == y1 { x1 } else { last_x };
        let mut row = String::new();
        for x in start..=end.min(last_x) {
            let Some(cell) = buffer.cell_mut(Position::new(x, y)) else {
                continue;
            };
            cell.set_bg(bg);
            row.push_str(cell.symbol());
        }
        if y > y0 {
            text.push('\n');
        }
        text.push_str(row.trim_end());
    }
    text
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

        let text = paint(
            &mut buffer,
            Rect::new(0, 0, 10, 3),
            Selection {
                anchor: (1, 0),
                head: (3, 0),
            },
            Color::DarkGray,
        );
        assert_eq!(text, "ell");

        let text = paint(
            &mut buffer,
            Rect::new(0, 0, 10, 3),
            Selection {
                anchor: (2, 1),
                head: (2, 0),
            },
            Color::DarkGray,
        );
        assert_eq!(text, "llo\nwor");
        assert_eq!(
            buffer.cell(Position::new(2, 0)).map(|cell| cell.bg),
            Some(Color::DarkGray),
            "the drag is painted where it read"
        );
    }
}
