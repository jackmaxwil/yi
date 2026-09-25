use std::error::Error;

use serde_json::Value;
use yi_orb::{self as orb, Mode, Opts};

type TestResult = Result<(), Box<dyn Error>>;

fn mode_of(name: &str) -> Mode {
    match name {
        "orbits" => Mode::Orbits,
        "globe" => Mode::Globe,
        "rubik" => Mode::Rubik,
        "wave" => Mode::Wave,
        "web" => Mode::Web,
        "braid" => Mode::Braid,
        "ribbon" => Mode::Ribbon,
        "ring" => Mode::Ring,
        _ => Mode::Morph,
    }
}

/// The library's own golden vectors (spec/orbs-golden.json, 72 cases,
/// 6-decimal, tolerance 1e-4): every dot of every state × size × timestamp
/// must match the reference engine as numbers — the same contract its
/// SwiftUI and React Native ports are held to.
#[test]
fn engine_matches_reference_golden_vectors() -> TestResult {
    let raw = include_str!("fixtures/orbs-golden.json");
    let golden: Value = serde_json::from_str(raw)?;
    let tolerance = golden["tolerance"].as_f64().ok_or("tolerance missing")?;
    let resolved = golden["resolved"].as_object().ok_or("resolved missing")?;
    let cases = golden["cases"].as_array().ok_or("cases missing")?;
    assert_eq!(cases.len(), 72);

    for case in cases {
        let key = case["key"].as_str().ok_or("key")?;
        let state_size = key.rsplit_once('-').map(|x| x.0).ok_or("key shape")?;
        let spec = &resolved[state_size];
        let mode = mode_of(spec["mode"].as_str().ok_or("mode")?);
        let opts_obj = spec["opts"].as_object().ok_or("opts")?;
        let mut map = std::collections::BTreeMap::new();
        for (name, value) in opts_obj {
            if let Some(number) = value.as_f64() {
                map.insert(leak(name), number);
            }
        }
        let opts = Opts(map);
        let size = case["size"].as_f64().ok_or("size")?;
        let t = case["t"].as_f64().ok_or("t")?;
        let frame = orb::frame(mode, size, t, &opts);

        let dots = case["dots"].as_array().ok_or("dots")?;
        let expected_dots = case["dotCount"].as_u64().ok_or("dotCount")? as usize;
        assert_eq!(frame.dots.len(), expected_dots, "{key}: dot count diverged");
        // Both sides sort by (z, x, y): coincident-z dots (the face-on ring
        // lives at z ≈ ±1 ulp) tie-break identically in either engine, and
        // draw order between truly coincident dots is visually meaningless.
        let mut reference: Vec<[f64; 6]> = dots
            .chunks(6)
            .map(|c| {
                let mut row = [f64::NAN; 6];
                for (slot, value) in row.iter_mut().zip(c) {
                    *slot = value.as_f64().unwrap_or(f64::NAN);
                }
                row
            })
            .collect();
        let mut ours: Vec<[f64; 6]> = frame
            .dots
            .iter()
            .map(|d| [d.x, d.y, d.z, d.r, d.white, d.a])
            .collect();
        // Quantize the sort key to the tolerance grid: last-ulp z noise
        // (±1e-16 around the face-on ring's z = 0) must not reorder dots
        // relative to each other differently in the two engines.
        let canon = |a: &[f64; 6], b: &[f64; 6]| {
            let key = |d: &[f64; 6]| {
                [
                    (d[2] / tolerance).round() as i64,
                    (d[0] / tolerance).round() as i64,
                    (d[1] / tolerance).round() as i64,
                    (d[3] / tolerance).round() as i64,
                ]
            };
            key(a).cmp(&key(b))
        };
        reference.sort_by(canon);
        ours.sort_by(canon);
        for (i, (mine, refr)) in ours.iter().zip(&reference).enumerate() {
            for (label, a, b) in [
                ("x", mine[0], refr[0]),
                ("y", mine[1], refr[1]),
                ("z", mine[2], refr[2]),
                ("r", mine[3], refr[3]),
                ("white", mine[4], refr[4]),
                ("a", mine[5], refr[5]),
            ] {
                assert!(
                    (a - b).abs() <= tolerance,
                    "{key} dot {i} {label}: {a} vs {b}"
                );
            }
        }
        let lines = case["lines"].as_array().ok_or("lines")?;
        let expected_lines = case["lineCount"].as_u64().ok_or("lineCount")? as usize;
        assert_eq!(frame.lines.len(), expected_lines, "{key}: line count");
        for (i, line) in frame.lines.iter().enumerate() {
            let base = i * 7;
            let expect = |o: usize| lines[base + o].as_f64().unwrap_or(f64::NAN);
            for (label, ours, reference) in [
                ("x1", line.x1, expect(0)),
                ("y1", line.y1, expect(1)),
                ("x2", line.x2, expect(2)),
                ("y2", line.y2, expect(3)),
                ("white", line.white, expect(4)),
                ("a", line.a, expect(5)),
                ("w", line.w, expect(6)),
            ] {
                assert!(
                    (ours - reference).abs() <= tolerance,
                    "{key} line {i} {label}: {ours} vs {reference}"
                );
            }
        }
    }
    Ok(())
}

fn leak(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

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
    let frame = orb::evaluate(orb::OrbState::Composing, 64, 1.234).ok_or("no frame")?;
    let rgba = orb::kitty::paint_rgba(&frame, 64.0, PX);

    let mut wire = Vec::new();
    orb::kitty::transmit(&mut wire, orb::kitty::IMAGE_IDS[0], &rgba, PX)?;
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
        wire.contains("\x1b[?2026h") && wire.contains("\x1b[?2026l"),
        "placement is bracketed in a synchronized update"
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

#[test]
fn orb_engine_feeds_the_kitty_painter() -> TestResult {
    use yi_orb::{OrbState, evaluate, kitty};
    let frame = evaluate(OrbState::Working, 64, 1.3).ok_or("preset missing")?;
    assert!(!frame.dots.is_empty());
    let rgba = kitty::paint_rgba(&frame, 64.0, 96);
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
