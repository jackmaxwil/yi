//! Kitty graphics for the notebook pane: base64 PNG payloads go out as `f=100` transmits and
//! a cell-rect placement, inside the synchronized frame so image and text land together.

use std::io::Write;

use ratatui::layout::Rect;

// Invariant: one placement is live at a time — the focused notebook's newest image, deleted
// on any change. A second concurrent image needs an id per placement, not this constant.
const IMAGE_ID: u32 = 7701;
const CHUNK: usize = 4096;

pub fn supported(term: Option<&str>, term_program: Option<&str>) -> bool {
    term.is_some_and(|term| term.contains("kitty") || term.contains("ghostty"))
        || term_program.is_some_and(|program| program == "ghostty")
}

/// Delete the previous placement, transmit the PNG, place it over `rect`. The payload is
/// already base64, the wire form the kernel attachment carries, so nothing decodes here.
pub fn place_png(out: &mut impl Write, base64_png: &str, rect: Rect) -> std::io::Result<()> {
    write!(out, "\u{1b}_Ga=d,d=i,i={IMAGE_ID},q=2\u{1b}\\")?;
    // Invariant (#817): a byte outside base64 could end this escape and start another; the
    // kernel refuses such data, but a session saved before it did still replays here.
    let base64 = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=');
    if !base64_png.bytes().all(base64) {
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    /// Incident (#817): a notebook image carrying `ESC \ ESC]52;…BEL` ended the transmit
    /// escape and made the terminal write the clipboard.
    #[test]
    fn a_payload_outside_base64_places_nothing() -> std::io::Result<()> {
        let rect = ratatui::layout::Rect::new(0, 0, 10, 5);
        let mut out = Vec::new();
        let payload = "AAAA\u{1b}\\\u{1b}]52;c;ZWNobyBwd25lZA==\u{7}";
        super::place_png(&mut out, payload, rect)?;
        assert_eq!(
            String::from_utf8_lossy(&out),
            "\u{1b}_Ga=d,d=i,i=7701,q=2\u{1b}\\",
            "only the delete of the image before it"
        );
        Ok(())
    }

    /// Pillow's 1x1 PNG, chosen so its base64 carries `+` and `/`, is transmitted and placed.
    #[test]
    fn a_base64_png_is_transmitted_and_placed() -> std::io::Result<()> {
        let rect = ratatui::layout::Rect::new(0, 0, 10, 5);
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";
        let mut out = Vec::new();
        super::place_png(&mut out, png, rect)?;
        let written = String::from_utf8_lossy(&out);
        assert!(
            written.contains(&format!("m=0;{png}\u{1b}\\")),
            "{written:?}"
        );
        assert!(written.contains("a=p,"), "{written:?}");
        Ok(())
    }
}
