use std::io::Write;

use super::core::OrbFrame;

/// Kitty graphics protocol support: Ghostty and kitty advertise via TERM or
/// KITTY_WINDOW_ID. Everything else falls back to the plain spinner line.
pub fn supported() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    term.contains("kitty")
        || term.contains("ghostty")
        || std::env::var_os("KITTY_WINDOW_ID").is_some()
}

/// TokyoNight-leaning light ink, matching the reference's monochrome
/// light-dots-on-dark contract; alpha carries depth over a transparent
/// background so the terminal's own ground (blur included) shows through.
const INK: (f64, f64, f64) = (0xc8 as f64, 0xd3 as f64, 0xf5 as f64);

/// Paint a finished frame to RGBA, the canvas painter's contract (dark
/// substrate: ink mirrored to 1 - white; lines first, dots far→near on top),
/// with a half-pixel feathered edge in place of canvas antialiasing.
pub fn paint_rgba(frame: &OrbFrame, canvas: f64, px: usize) -> Vec<u8> {
    let scale = px as f64 / canvas;
    let mut buf = vec![0.0_f64; px * px * 4];
    let mut blend = |x: usize, y: usize, ink: f64, alpha: f64| {
        let index = (y * px + x) * 4;
        let Some(slot) = buf.get_mut(index..index + 4) else {
            return;
        };
        let src_a = alpha.clamp(0.0, 1.0);
        let (r, g, b) = (INK.0 * ink, INK.1 * ink, INK.2 * ink);
        for (channel, value) in [r, g, b].into_iter().enumerate() {
            slot[channel] = value * src_a + slot[channel] * (1.0 - src_a);
        }
        slot[3] = (src_a + slot[3] * (1.0 - src_a)).min(1.0);
    };

    for line in &frame.lines {
        let ink = 1.0 - line.white.clamp(0.0, 1.0);
        let (x1, y1) = (line.x1 * scale, line.y1 * scale);
        let (x2, y2) = (line.x2 * scale, line.y2 * scale);
        let w = (line.w * scale).max(1.0);
        let steps = ((x2 - x1).abs().max((y2 - y1).abs()).ceil() as usize).max(1);
        for step in 0..=steps {
            let f = step as f64 / steps as f64;
            let cx = x1 + (x2 - x1) * f;
            let cy = y1 + (y2 - y1) * f;
            stamp(&mut blend, px, cx, cy, w / 2.0, ink, line.a);
        }
    }
    for dot in &frame.dots {
        let ink = 1.0 - dot.white.clamp(0.0, 1.0);
        stamp(
            &mut blend,
            px,
            dot.x * scale,
            dot.y * scale,
            dot.r * scale,
            ink,
            dot.a,
        );
    }

    buf.into_iter().map(|v| (v * 255.0).round() as u8).collect()
}

fn stamp(
    blend: &mut impl FnMut(usize, usize, f64, f64),
    px: usize,
    cx: f64,
    cy: f64,
    r: f64,
    ink: f64,
    alpha: f64,
) {
    let reach = (r + 1.0).ceil() as i64;
    let (icx, icy) = (cx.round() as i64, cy.round() as i64);
    for oy in -reach..=reach {
        for ox in -reach..=reach {
            let (x, y) = (icx + ox, icy + oy);
            if x < 0 || y < 0 || x >= px as i64 || y >= px as i64 {
                continue;
            }
            let dist = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            let coverage = (r - dist + 0.5).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(x as usize, y as usize, ink, alpha * coverage);
            }
        }
    }
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let keep = chunk.len() + 1;
        for index in 0..4 {
            if index < keep {
                let slot = ((bits >> (18 - 6 * index)) & 0x3f) as usize;
                out.push(char::from(TABLE.get(slot).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

const IMAGE_ID: u32 = 7601;

/// Transmit and place one RGBA frame at a cell rect: cursor saved, moved to
/// (row, col) 0-based, image scaled into `cols` × `rows` cells, cursor
/// restored. Same image id every frame, so kitty replaces rather than
/// accumulates; payload chunked at 4096 as the protocol requires.
pub fn emit(
    out: &mut impl Write,
    rgba: &[u8],
    px: usize,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let payload = base64(rgba);
    // Placements scroll with the text under them (insert_before pushes the
    // old one up into the transcript) — delete every placement of the id
    // before placing again, or each frame leaves a stamp behind.
    write!(out, "\x1b_Ga=d,d=i,i={IMAGE_ID},q=2\x1b\\")?;
    write!(out, "\x1b7\x1b[{};{}H", row + 1, col + 1)?;
    let chunks: Vec<&str> = payload
        .as_bytes()
        .chunks(4096)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            write!(
                out,
                "\x1b_Gf=32,s={px},v={px},a=T,i={IMAGE_ID},p=1,q=2,C=1,c={cols},r={rows},m={more};{chunk}\x1b\\"
            )?;
        } else {
            write!(out, "\x1b_Gm={more};{chunk}\x1b\\")?;
        }
    }
    write!(out, "\x1b8")?;
    out.flush()
}

/// Remove the orb image (turn ended, or the fallback path takes over).
pub fn delete(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\x1b_Ga=d,d=i,i={IMAGE_ID},q=2\x1b\\")?;
    out.flush()
}
