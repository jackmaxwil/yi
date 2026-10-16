//! Kitty graphics for the notebook and browser panes: base64 PNG payloads go out as `f=100`
//! transmits and a cell-rect placement, inside the synchronized frame with the text.

use std::io::Write;

use ratatui::layout::Rect;

/// Incident: ids 0 and 1 alternated once, and kitty id 0 means "no id": Ghostty refuses to place
/// it and keeps the transmit. A repaint alternates these, so it never deletes the id it placed.
const NOTEBOOK_IDS: [u32; 2] = [7701, 7702];

pub(crate) fn next_notebook_id(old: Option<u32>) -> u32 {
    let [even, odd] = NOTEBOOK_IDS;
    if old == Some(even) { odd } else { even }
}
const CHUNK: usize = 4096;

pub fn supported(term: Option<&str>, term_program: Option<&str>) -> bool {
    term.is_some_and(|term| term.contains("kitty") || term.contains("ghostty"))
        || term_program.is_some_and(|program| program == "ghostty")
}

/// Transmit the PNG and place `id` over `rect`. The payload is already base64, the wire
/// form the kernel attachment carries, so nothing decodes here.
pub fn place_png(
    out: &mut impl Write,
    id: u32,
    base64_png: &str,
    rect: Rect,
) -> std::io::Result<()> {
    // Invariant (#817): a byte outside base64 could end this escape and start another; the
    // kernel refuses such data, but a session saved before it did still replays here.
    if !yi_types::image::is_strict_base64(base64_png) {
        return Ok(());
    }
    transmit_base64(out, id, base64_png)?;
    yi_tui::orb::kitty::place(
        out,
        id,
        rect.x,
        rect.y,
        rect.width.max(1),
        rect.height.max(1),
    )
}

/// Transmits base64 its caller already checked with [`yi_types::image::is_strict_base64`].
pub fn transmit_base64(out: &mut impl Write, id: u32, base64_png: &str) -> std::io::Result<()> {
    let bytes = base64_png.as_bytes();
    let mut offset = 0;
    let mut first = true;
    while offset < bytes.len() {
        let end = offset.saturating_add(CHUNK).min(bytes.len());
        let chunk = bytes.get(offset..end).unwrap_or_default();
        let more = u8::from(end < bytes.len());
        if first {
            write!(out, "\u{1b}_Gf=100,a=t,i={id},q=2,m={more};")?;
            first = false;
        } else {
            write!(out, "\u{1b}_Gm={more};")?;
        }
        out.write_all(chunk)?;
        write!(out, "\u{1b}\\")?;
        offset = end;
    }
    Ok(())
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
        super::place_png(&mut out, super::NOTEBOOK_IDS[0], payload, rect)?;
        assert!(out.is_empty(), "no transmit or placement escape: {out:?}");
        Ok(())
    }

    /// Pillow's 1x1 PNG, chosen so its base64 carries `+` and `/`, is transmitted and placed.
    #[test]
    fn a_base64_png_is_transmitted_and_placed() -> std::io::Result<()> {
        let rect = ratatui::layout::Rect::new(0, 0, 10, 5);
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";
        let mut out = Vec::new();
        super::place_png(&mut out, super::NOTEBOOK_IDS[0], png, rect)?;
        let written = String::from_utf8_lossy(&out);
        assert!(
            written.contains(&format!("m=0;{png}\u{1b}\\")),
            "{written:?}"
        );
        assert!(written.contains("a=p,"), "{written:?}");
        Ok(())
    }
}
