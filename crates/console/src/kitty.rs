//! Kitty graphics for the notebook pane: base64 PNG payloads go out as
//! `f=100` transmits and a cell-rect placement, inside the synchronized
//! frame so the image lands with the text it belongs to.

use std::io::Write;

use ratatui::layout::Rect;

// Invariant: one placement is live at a time. `place_notebook_image` draws
// the focused notebook's newest image only and deletes on any change, so a
// second concurrent image needs an id per placement, not this constant.
const IMAGE_ID: u32 = 7701;
const CHUNK: usize = 4096;

pub fn supported(term: Option<&str>, term_program: Option<&str>) -> bool {
    term.is_some_and(|term| term.contains("kitty") || term.contains("ghostty"))
        || term_program.is_some_and(|program| program == "ghostty")
}

/// Delete the previous placement, transmit the PNG, place it over `rect`.
/// The payload is already base64 (the wire form the kernel attachment
/// carries), so this never decodes image data.
pub fn place_png(out: &mut impl Write, base64_png: &str, rect: Rect) -> std::io::Result<()> {
    write!(out, "\u{1b}_Ga=d,d=i,i={IMAGE_ID},q=2\u{1b}\\")?;
    let bytes = base64_png.as_bytes();
    let mut offset = 0;
    let mut first = true;
    while offset < bytes.len() {
        let end = offset.saturating_add(CHUNK).min(bytes.len());
        let chunk = bytes.get(offset..end).unwrap_or_default();
        let more = u8::from(end < bytes.len());
        if first {
            write!(out, "\u{1b}_Gf=100,a=t,i={IMAGE_ID},q=2,m={more};")?;
            first = false;
        } else {
            write!(out, "\u{1b}_Gm={more};")?;
        }
        out.write_all(chunk)?;
        write!(out, "\u{1b}\\")?;
        offset = end;
    }
    let row = rect.y.saturating_add(1);
    let col = rect.x.saturating_add(1);
    let rows = rect.height.max(1);
    let cols = rect.width.max(1);
    write!(
        out,
        "\u{1b}7\u{1b}[{row};{col}H\u{1b}_Ga=p,i={IMAGE_ID},r={rows},c={cols},q=2\u{1b}\\\u{1b}8"
    )
}

pub fn delete(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\u{1b}_Ga=d,d=i,i={IMAGE_ID},q=2\u{1b}\\")
}
