use std::error::Error;

use serde_json::Value;
use yi_tui::orb::{self, Mode, Opts};

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
