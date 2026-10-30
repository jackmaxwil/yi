use std::error::Error;

use yi_orb as orb;

type TestResult = Result<(), Box<dyn Error>>;

/// §17.3 `o=z`: the terminal reads a zlib stream, not raw RGBA. A drift in the
/// control keys, the chunk bound, or the compressed payload leaves the orb
/// undrawn on every kitty-family terminal, and no non-kitty test path can see
/// it. Ground truth is RFC 1950's own header rule plus the painted bytes —
/// not this compressor's opinion of its own output. P9: transmit (`a=t`)
/// carries no delete and no placement — placement is its own escape, so a
/// frame swap never shows an empty cell.
#[test]
fn kitty_emit_transmits_an_inflatable_zlib_stream() -> TestResult {
    const PX: usize = 192;
    let frame = orb::render(&orb::pose(Some(orb::OrbState::Composing), 1.234));
    let rgba = orb::kitty::paint_rgba(&frame, 64.0, PX, PX);

    let mut wire = Vec::new();
    orb::kitty::transmit(&mut wire, orb::kitty::IMAGE_IDS[0], &rgba, (PX, PX))?;
    orb::kitty::place(&mut wire, orb::kitty::IMAGE_IDS[0], 3, 7, 6, 3)?;
    let wire = String::from_utf8(wire)?;

    let (head, tail) = wire.split_once("\x1b_Gf=32,").ok_or("no transmit escape")?;
    assert!(
        !head.contains("a=d"),
        "a frame swap must never delete first"
    );
    let (control, tail) = tail.split_once(';').ok_or("no payload separator")?;
    assert!(
        control.contains("a=t"),
        "transmit displays nothing: {control}"
    );
    assert!(
        control.contains("o=z"),
        "payload is compressed but not declared: {control}"
    );
    assert!(control.contains(&format!("s={PX},v={PX}")), "{control}");
    let place_at = wire.find("a=p,").ok_or("no placement escape")?;
    let placement = wire.get(place_at..).unwrap_or_default();
    assert!(placement.starts_with("a=p,i=7601,p=1"), "{placement}");
    assert!(
        !wire.contains("\x1b[?2026"),
        "a placement opens no update of its own: the mode does not nest, so its end shows the \
         caller's frame early"
    );
    assert!(
        wire.contains("\x1b7") && wire.contains("\x1b8"),
        "cursor saved and restored around the placement"
    );

    let mut payload = String::new();
    let (first, mut rest) = tail.split_once("\x1b\\").ok_or("unterminated chunk")?;
    let mut chunk = first;
    let mut more = control.ends_with("m=1");
    loop {
        assert!(chunk.len() <= 4096, "chunk over the protocol bound");
        payload.push_str(chunk);
        if !more {
            break;
        }
        let (escape, next) = rest.split_once("\x1b\\").ok_or("unterminated chunk")?;
        let (keys, body) = escape
            .strip_prefix("\x1b_G")
            .ok_or("chunk is not a graphics escape")?
            .split_once(';')
            .ok_or("no payload separator")?;
        (more, chunk, rest) = (keys == "m=1", body, next);
    }

    let raw = decode_base64(&payload)?;
    // RFC 1950 §2.2: low nibble of CMF is the compression method (8 = deflate)
    // and the two header bytes read big-endian must be a multiple of 31.
    let (cmf, flg) = (
        *raw.first().ok_or("empty stream")?,
        *raw.get(1).ok_or("truncated header")?,
    );
    assert_eq!(cmf & 0x0f, 8, "not a deflate stream");
    assert_eq!((u16::from(cmf) << 8 | u16::from(flg)) % 31, 0, "bad FCHECK");

    let inflated = miniz_oxide::inflate::decompress_to_vec_zlib(&raw)
        .map_err(|error| format!("inflate failed: {error:?}"))?;
    assert_eq!(inflated, rgba, "the terminal would paint different pixels");
    assert!(
        raw.len() * 4 < rgba.len(),
        "compression under 4x — {} from {}",
        raw.len(),
        rgba.len()
    );
    Ok(())
}

#[test]
fn base64_payloads_leave_in_bounded_chunks() -> TestResult {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";
    for len in [4096, 4097, 10000] {
        let payload: String = PNG.chars().cycle().take(len).collect();
        let mut wire = Vec::new();
        orb::kitty::transmit_base64(&mut wire, 42, &payload, "f=100")?;
        let wire = String::from_utf8(wire)?;
        let escapes: Vec<&str> = wire
            .split("\x1b\\")
            .filter(|part| !part.is_empty())
            .collect();
        assert_eq!(escapes.len(), len.div_ceil(4096), "{len} bytes");
        let mut offset = 0_usize;
        for (index, escape) in escapes.into_iter().enumerate() {
            let (control, body) = escape
                .strip_prefix("\x1b_G")
                .ok_or("no APC")?
                .split_once(';')
                .ok_or("no payload")?;
            let end = offset.saturating_add(4096).min(len);
            let more = u8::from(end < len);
            let expected = if index == 0 {
                format!("f=100,a=t,i=42,q=2,m={more}")
            } else {
                format!("m={more}")
            };
            assert_eq!(control, expected, "{len} bytes, chunk {index}");
            assert_eq!(body, payload.get(offset..end).ok_or("payload boundary")?);
            offset = end;
        }
        assert_eq!(offset, len);
    }
    Ok(())
}

fn decode_base64(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for quad in text.as_bytes().chunks(4) {
        let mut bits = 0u32;
        let mut keep = 0usize;
        for (index, byte) in quad.iter().enumerate() {
            if *byte == b'=' {
                continue;
            }
            let slot = TABLE
                .iter()
                .position(|candidate| candidate == byte)
                .ok_or("non-base64 byte in payload")?;
            bits |= (slot as u32) << (18 - 6 * index);
            keep = index;
        }
        out.extend_from_slice(&bits.to_be_bytes()[1..=keep]);
    }
    Ok(out)
}

/// Invariant: TMUX is a socket path under the user's TMUX_TMPDIR and may hold bytes that are
/// not UTF-8; dropping to a value parse writes raw APC into the multiplexer that swallows it.
#[cfg(unix)]
#[test]
fn a_multiplexer_variable_outside_utf8_still_falls_back_to_text() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let tmux = OsStr::from_bytes(b"/tmp/tmux-\xff/default,42,0").to_os_string();
    let var = |key: &str| -> Option<std::ffi::OsString> {
        match key {
            "TERM" => Some("xterm-kitty".into()),
            "TMUX" => Some(tmux.clone()),
            _ => None,
        }
    };
    assert!(
        !orb::kitty::supported_with(var),
        "raw APC escapes reach the multiplexer"
    );
}

#[test]
fn orb_engine_feeds_the_kitty_painter() -> TestResult {
    use yi_orb::{OrbState, kitty, pose, render};
    let frame = render(&pose(Some(OrbState::Working), 1.3));
    assert!(!frame.dots.is_empty());
    let rgba = kitty::paint_rgba(&frame, 64.0, 96, 96);
    assert_eq!(rgba.len(), 96 * 96 * 4);
    let lit = rgba.chunks(4).filter(|px| px[3] > 0).count();
    assert!(
        lit > 200,
        "a working orb must light pixels with alpha depth: {lit}"
    );
    let background = rgba.chunks(4).filter(|px| px[3] == 0).count();
    assert!(
        background > 1000,
        "the background stays transparent for the terminal ground: {background}"
    );
    Ok(())
}

/// Incident: ink scaled by 255 twice saturated every lit pixel to white. Straight alpha keeps
/// a dot's soft edge its own colour instead of darkening toward black.
#[test]
fn a_dot_paints_its_ink_to_the_edge() -> TestResult {
    use yi_orb::core::Dot;
    let frame = orb::OrbFrame {
        dots: vec![Dot {
            x: 32.0,
            y: 32.0,
            z: 0.0,
            r: 8.0,
            white: 0.5,
            a: 1.0,
        }],
    };
    let rgba = orb::kitty::paint_rgba(&frame, 64.0, 64, 64);
    let (r, g, b) = orb::kitty::INK_RGB;
    let ink = [r, g, b].map(|channel| (f64::from(channel) * 0.5).round() as i16);
    let centre = rgba
        .get((32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4)
        .ok_or("centre pixel")?;
    assert_eq!(
        centre.get(3),
        Some(&255),
        "the centre is opaque: {centre:?}"
    );
    let edges = rgba.chunks(4).filter(|px| px[3] > 0 && px[3] < 255).count();
    assert!(edges > 0, "the edge is antialiased");
    for px in rgba.chunks(4).filter(|px| px[3] > 0) {
        for (channel, want) in px.iter().zip(ink) {
            assert!(
                (i16::from(*channel) - want).abs() <= 1,
                "every lit pixel keeps the dot's ink, want {ink:?}: {px:?}"
            );
        }
    }
    Ok(())
}
