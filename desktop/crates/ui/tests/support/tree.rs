//! Assertions over a drawn element tree: find by key or text, rectangles, rows, sizes, goldens.

use std::error::Error;
use std::fmt::Write as _;
use std::path::PathBuf;

use rune::ElementInfo;

pub type Check = Result<(), Box<dyn Error>>;

/// Set to `1` to rewrite every golden a run compares instead of comparing it.
pub const BLESS: &str = "YD_BLESS";

pub fn by_key<'a>(tree: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    tree.iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key))
        .ok_or_else(|| format!("no element keyed {key:?}").into())
}

pub fn by_text<'a>(tree: &'a [ElementInfo], text: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    tree.iter()
        .find(|element| !element.exiting && element.text.as_deref() == Some(text))
        .ok_or_else(|| format!("no element shows {text:?}").into())
}

/// `[x, y, w, h]` in window points.
pub fn rect(tree: &[ElementInfo], key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    Ok(by_key(tree, key)?.rect)
}

/// The words drawn inside the keyed element, left to right, joined by one space.
pub fn row_text(tree: &[ElementInfo], key: &str) -> Result<String, Box<dyn Error>> {
    let [x, y, w, h] = rect(tree, key)?;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let mut words: Vec<(f32, &str)> = tree
        .iter()
        .filter(|element| !element.exiting && inside(&element.rect))
        .filter_map(|element| Some((element.rect[0], element.text.as_deref()?)))
        .collect();
    words.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(words
        .into_iter()
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" "))
}

pub fn assert_row(tree: &[ElementInfo], key: &str, want: &str) -> Check {
    let got = row_text(tree, key)?;
    if got == want {
        return Ok(());
    }
    Err(format!("row {key:?}\n  want: {want:?}\n  got:  {got:?}").into())
}

/// Width and height of the keyed element each within `tolerance` points of `want`.
pub fn assert_size(tree: &[ElementInfo], key: &str, want: (f32, f32), tolerance: f32) -> Check {
    let [_, _, w, h] = rect(tree, key)?;
    if (w - want.0).abs() <= tolerance && (h - want.1).abs() <= tolerance {
        return Ok(());
    }
    Err(format!(
        "{key:?} is {w} x {h}, want {} x {} within {tolerance}",
        want.0, want.1
    )
    .into())
}

/// One line per element: what a golden file holds.
pub fn render(tree: &[ElementInfo]) -> String {
    let mut out = String::new();
    for e in tree.iter().filter(|element| !element.exiting) {
        let [x, y, w, h] = e.rect;
        let _ = write!(
            out,
            "{:?} {} [{x:.2} {y:.2} {w:.2} {h:.2}] {}",
            e.path,
            e.role,
            e.fill.hex()
        );
        if let Some(key) = &e.key {
            let _ = write!(out, " key={key:?}");
        }
        if let Some(text) = &e.text {
            let _ = write!(out, " text={text:?}");
        }
        if let Some(colour) = e.text_color {
            let _ = write!(out, " colour={}", colour.hex());
        }
        out.push('\n');
    }
    out
}

/// Compares the tree with `golden` (relative to the crate); [`BLESS`] rewrites it instead.
pub fn assert_golden(tree: &[ElementInfo], golden: &str) -> Check {
    let bless = std::env::var(BLESS).is_ok_and(|value| value == "1");
    compare(&render(tree), golden, bless)
}

/// On a difference the fresh text is written under the target folder, named in the error.
pub fn compare(fresh: &str, golden: &str, bless: bool) -> Check {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(golden);
    if bless {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&path, fresh)?;
        return Ok(());
    }
    let want = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "golden {}: {error}; write it with {BLESS}=1",
            path.display()
        )
    })?;
    if want == fresh {
        return Ok(());
    }
    let name = path.file_name().unwrap_or_default();
    let beside = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&beside, fresh)?;
    let line = want
        .lines()
        .zip(fresh.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| want.lines().count().min(fresh.lines().count()));
    Err(format!(
        "golden differs at line {}\n  golden: {}\n  fresh:  {}\n  accept with {BLESS}=1 after review",
        line.saturating_add(1),
        path.display(),
        beside.display()
    )
    .into())
}
