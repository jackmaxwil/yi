use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::core::OrbFrame;
use crate::colors::Theme;

// Braille cell bit layout (U+2800 + mask): columns × rows
// (0,0)=0x01 (0,1)=0x02 (0,2)=0x04 (0,3)=0x40
// (1,0)=0x08 (1,1)=0x10 (1,2)=0x20 (1,3)=0x80
const BIT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// The dark-theme paint contract: ink is mirrored (1 - white), alpha scales
/// it — one brightness per grid dot, the brightest dot in a cell picks the
/// cell's tier.
fn brightness(white: f64, a: f64) -> f64 {
    (1.0 - white.clamp(0.0, 1.0)) * a.clamp(0.0, 1.0)
}

/// Rasterize an orb frame (canvas units `size` × `size`) onto a
/// `cols` × `rows` braille grid: 2×4 dots per cell, ink quantized to the
/// theme's dim/muted/text tiers.
pub fn render(
    frame: &OrbFrame,
    size: f64,
    cols: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let grid_w = cols * 2;
    let grid_h = rows * 4;
    let sx = grid_w as f64 / size;
    let sy = grid_h as f64 / size;
    let mut mask = vec![vec![0_u8; cols]; rows];
    let mut bright = vec![vec![0.0_f64; cols]; rows];

    let mut plot = |gx: i64, gy: i64, level: f64| {
        if gx < 0 || gy < 0 {
            return;
        }
        let (gx, gy) = (gx as usize, gy as usize);
        if gx >= grid_w || gy >= grid_h {
            return;
        }
        let (cell_x, sub_x) = (gx / 2, gx % 2);
        let (cell_y, sub_y) = (gy / 4, gy % 4);
        mask[cell_y][cell_x] |= BIT[sub_x][sub_y];
        if level > bright[cell_y][cell_x] {
            bright[cell_y][cell_x] = level;
        }
    };

    // lines first (the web's edges), nodes on top — same order as paintFrame
    for line in &frame.lines {
        let level = brightness(line.white, line.a) * 0.8;
        if level < 0.08 {
            continue;
        }
        let (x1, y1) = (line.x1 * sx, line.y1 * sy);
        let (x2, y2) = (line.x2 * sx, line.y2 * sy);
        let steps = (x2 - x1).abs().max((y2 - y1).abs()).ceil() as usize + 1;
        for step in 0..=steps {
            let f = step as f64 / steps as f64;
            let gx = (x1 + (x2 - x1) * f).round() as i64;
            let gy = (y1 + (y2 - y1) * f).round() as i64;
            plot(gx, gy, level);
        }
    }
    for dot in &frame.dots {
        let level = brightness(dot.white, dot.a);
        if level < 0.08 {
            continue;
        }
        let gx = dot.x * sx;
        let gy = dot.y * sy;
        let gr = (dot.r * sx).max(0.0);
        if gr < 0.75 {
            plot(gx.round() as i64, gy.round() as i64, level);
        } else {
            let reach = gr.ceil() as i64;
            for oy in -reach..=reach {
                for ox in -reach..=reach {
                    let fx = ox as f64;
                    let fy = oy as f64;
                    if fx * fx + fy * fy <= gr * gr {
                        plot(gx.round() as i64 + ox, gy.round() as i64 + oy, level);
                    }
                }
            }
        }
    }

    let tiers = [
        (0.55, Style::default().fg(theme.text)),
        (0.3, theme.muted_style()),
        (0.0, theme.dim_style()),
    ];
    (0..rows)
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(cols);
            for col in 0..cols {
                let bits = mask[row][col];
                if bits == 0 {
                    spans.push(Span::raw(" "));
                    continue;
                }
                let ch = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
                let level = bright[row][col];
                let style = tiers
                    .iter()
                    .find(|(floor, _)| level >= *floor)
                    .map(|(_, style)| *style)
                    .unwrap_or_default();
                spans.push(Span::styled(ch.to_string(), style));
            }
            Line::from(spans)
        })
        .collect()
}
