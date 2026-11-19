//! The machines source's asks, the Remote desktop pane's buttons, and the one recorded screen
//! that the source replays and the pane's scenes draw, so neither keeps a copy of the other.

use serde_json::Value as Json;

use crate::ids::CommandId;
use crate::row::Row;

/// Names the adapter that answers: `none` or `recorded`, as the text.
pub const ADAPTER: CommandId = CommandId("remote.adapter");
/// Shows a machine's screen; aimed at its `vm` address, or at none for the first.
pub const OPEN: CommandId = CommandId("remote.open");
pub const WATCH: CommandId = CommandId("remote.watch");
pub const CONTROL: CommandId = CommandId("remote.control");
pub const MARK_UP: CommandId = CommandId("remote.mark-up");
pub const SCREENSHOT: CommandId = CommandId("remote.screenshot");

/// What both lists and the screen say while no adapter is named.
pub const NO_HOST: &str = "No machine host is set up. Name one in Settings.";
/// What a command, or input on the screen, is answered with on recorded data.
pub const RECORDED_ONLY: &str = "not available on recorded data";

/// The machine whose screen was recorded: the only one with a picture.
pub const RECORDED_MACHINE: &str = "ci-ubuntu";
/// The record models every remote screen at this size, in its own pixels.
pub const SCREEN: (u32, u32) = (860, 484);
/// Where the recorded desktop's windows and rows sit, from the mockup's own geometry.
pub const TOP_BAR: u32 = 22;
pub const TITLE_BAR: u32 = 24;
pub const UPLOAD_ROW: u32 = 42;
pub const LOG_LINE: u32 = 17;

const DESKTOP: &str = include_str!("../../../../../fixtures/mockup/panes/desktop.text.json");
/// How far each upload's bar is filled, in the record's order.
const PROGRESS: [u32; 4] = [100, 100, 62, 0];
/// A rectangle of the screen in its own pixels: left, top, width, height.
type Rect = (u32, u32, u32, u32);
const APP: (&str, Rect) = ("atlas-uploader", (56, 54, 380, 296));
const TERMINAL: (&str, Rect) = ("terminal", (396, 196, 420, 236));

/// The recorded screen: its rows as the source sends them, and a picture of the bare desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub rows: Vec<Row>,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn strings() -> Result<Vec<String>, String> {
    let json: Json = serde_json::from_str(DESKTOP).map_err(|why| why.to_string())?;
    let list = json.get("strings").and_then(Json::as_array);
    let list = list.ok_or("the desktop fixture lists no strings")?;
    let words = list.iter().filter_map(|word| word.as_str());
    Ok(words.map(str::to_owned).collect())
}

/// The log's lines: the fixture holds them as one string, each starting `$` or a clock time.
fn log_lines(text: &str) -> Vec<String> {
    let clock = |word: &str| word.len() == 8 && word.matches(':').count() == 2;
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if word != "$" && !clock(word) => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

fn placed(row: Row, (x, y, w, h): Rect) -> Row {
    let row = row.text("x", x.to_string()).text("y", y.to_string());
    row.text("w", w.to_string()).text("h", h.to_string())
}

fn halves(words: &str) -> (&str, &str) {
    words.split_once(" · ").unwrap_or_default()
}

/// The sample screen as the desktop fixture words it.
pub fn sample() -> Result<Sample, String> {
    let s = strings()?;
    let at = |i: usize| s.get(i).map_or("", String::as_str);
    let (fps, took) = halves(at(1));
    let (size, _) = halves(at(2));
    let time = at(6).rsplit(' ').next().unwrap_or_default();
    let mut rows = vec![
        Row::new("screen:0")
            .text("machine", at(0))
            .text("size", size)
            .text("fps", fps)
            .text("ms", took.trim_end_matches(" ms"))
            .text("time", time)
            .text("note", ""),
    ];
    let top = (0, 0, SCREEN.0, TOP_BAR);
    for (n, align) in ["left", "center", "right"].into_iter().enumerate() {
        let row = Row::new(format!("top:{align}")).text("kind", "bar");
        let name = at(5usize.saturating_add(n));
        rows.push(placed(row.text("name", name).text("align", align), top));
    }
    for (id, rect) in [APP, TERMINAL] {
        let row = Row::new(format!("window:{id}")).text("kind", "window");
        let title = if id == APP.0 { at(8) } else { at(17) };
        rows.push(placed(row.text("name", title), rect));
    }
    let (x, y, w, _) = APP.1;
    let (mut status, mut upload) = ("", 0u32);
    for (i, name) in s.iter().enumerate().take(16).skip(9) {
        if !name.ends_with(".png") {
            continue;
        }
        let next = s.get(i.saturating_add(1));
        status = next.filter(|n| !n.ends_with(".png")).map_or(status, |n| n);
        let top = y + TITLE_BAR + UPLOAD_ROW * upload;
        let fill = PROGRESS.get(upload as usize).copied().unwrap_or(0);
        let row = Row::new(format!("item:{}", name.trim_end_matches(".png")));
        let row = row.text("kind", "row").text("name", name.as_str());
        let row = row.text("status", status).text("fill", fill.to_string());
        rows.push(placed(row.text("parent", APP.0), (x, top, w, UPLOAD_ROW)));
        upload = upload.saturating_add(1);
    }
    let (x, y, w, _) = TERMINAL.1;
    let lines = log_lines(at(18));
    let last = lines.len().saturating_sub(1);
    for (n, line) in lines.iter().enumerate() {
        let top = y + TITLE_BAR + 8 + LOG_LINE * u32::try_from(n).unwrap_or(0);
        let row = Row::new(format!("line:{n}")).text("kind", "line");
        let row = row.text("name", line.as_str()).text("parent", TERMINAL.0);
        let row = row.text("warn", if n == last { "yes" } else { "no" });
        rows.push(placed(row, (x + 10, top, w - 20, LOG_LINE)));
    }
    let pointer = Row::new("pointer:0").text("kind", "pointer");
    let (px, py) = (APP.1.0 + 250, APP.1.1 + 150);
    rows.push(placed(pointer.text("name", at(16)), (px, py, 0, 0)));
    let (width, height) = SCREEN;
    Ok(Sample {
        rows,
        width,
        height,
        rgba: desktop(width, height),
    })
}

/// A 160 degree blue-grey gradient: the bare desktop under the windows the pane draws.
fn desktop(width: u32, height: u32) -> Vec<u8> {
    let (from, to): ([u8; 3], [u8; 3]) = ([0x4a, 0x55, 0x68], [0x2c, 0x34, 0x42]);
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            // The gradient runs along (sin 160, -cos 160) over the whole box.
            let along = (x as f32 - 430.0) * 0.342 + (y as f32 - 242.0) * 0.940;
            let t = (0.5 + along / 749.0).clamp(0.0, 1.0);
            for (a, b) in from.into_iter().zip(to) {
                rgba.push((f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8);
            }
            rgba.push(255);
        }
    }
    rgba
}
