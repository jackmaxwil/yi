//! The IPython pane, driven through the window from a real server's recorded turn of six
//! kernel cells: streams, Markdown, a DataFrame, a chart, 3,000 printed lines and an error.

use yd_core::grid::saved::BLANK;
use yd_core::ids::ThreadId;
use yd_core::pane::PaneKind;
use yd_core::wire::replay::play;
use yd_ui::grid_view::view::{Act, act, show_pane};

use super::*;

const TURN: &str = include_str!("../../../../../../fixtures/wire/kernel-cells.jsonl");
/// A turn of the faux model with no ipython call.
const PLAIN: &str = include_str!("../../../../../../fixtures/wire/turn.jsonl");
/// The recording's lines up to the third cell's start, and up to its end.
const THIRD_STARTED: usize = 87;
const THIRD_ENDED: usize = 93;

fn lines(recording: &str, from: usize, to: usize) -> String {
    let lines = recording.lines().skip(from).take(to.saturating_sub(from));
    lines.collect::<Vec<_>>().join("\n")
}

/// The thread a recording made: what its `session/new` answered.
fn made(recording: &str) -> Result<ThreadId, Box<dyn Error>> {
    let answer = recording
        .lines()
        .find(|line| line.contains("\"result\":{\"sessionId\""));
    let answer = answer.ok_or("no session/new answer")?;
    let at = answer.find("\"sessionId\":\"").ok_or("no session")? + 13;
    let id = answer.get(at..at + 36).ok_or("short id")?;
    Ok(ThreadId::new(id))
}

/// The app's window over the first `to` recorded lines, its thread shown, chat alone.
fn window(recording: &str, to: usize) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    let thread = made(recording)?;
    {
        let mut hub = win.hub.try_borrow_mut()?;
        play(&mut hub.model, &lines(recording, 0, to))?;
        yd_ui::shown::pick(&mut hub, root::MAIN, thread);
    }
    let mut runtime = Runtime::new(win, root::view, (1600.0, 900.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    runtime.update(|win, tx| act(win, tx, Act::Show(BLANK.slug)));
    rest(&mut runtime);
    Ok(runtime)
}

/// The same window with the IPython pane shown beside the chat; its key.
fn notebook(recording: &str, to: usize) -> Result<(Runtime<Win>, String), Box<dyn Error>> {
    let mut runtime = window(recording, to)?;
    runtime.update(|win, tx| {
        show_pane(win, tx, PaneKind::IPython);
    });
    rest(&mut runtime);
    let key = pane(&runtime)?;
    Ok((runtime, key))
}

fn pane(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let found = shown
        .iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key.clone());
    let mut found =
        found.filter(|key| key.starts_with("ipython-") && key.ends_with(".pane-header"));
    let found = found.next().ok_or("no IPython pane")?;
    Ok(found.trim_end_matches(".pane-header").to_owned())
}

/// Scrolls the cells until `key` is in view, as a reader would before pressing it.
fn reveal(runtime: &mut Runtime<Win>, key: &str) {
    let key = key.to_owned();
    runtime.update(move |_, tx| tx.scroll_into_view(key.clone()));
    rest(runtime);
}

fn words(runtime: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    row_text(&runtime.elements(), key)
}

fn rect(runtime: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let shown = runtime.elements();
    Ok(by_key(&shown, key)
        .ok_or_else(|| format!("no element {key}"))?
        .rect)
}

/// Each kind of output is drawn the way a notebook draws it, under the cell that made it.
#[test]
fn each_recorded_cell_shows_its_code_and_its_outputs_by_kind() -> Check {
    let (runtime, key) = notebook(TURN, usize::MAX)?;
    let head = words(&runtime, &format!("{key}.pane-header"))?;
    assert!(
        head.ends_with("6 cells"),
        "the header counts the cells: {head:?}"
    );
    let cell = |call: &str, part: &str| format!("{key}.cell.{call}.{part}");
    let input = words(&runtime, &cell("k1", "input"))?;
    assert!(input.starts_with("In import sys"), "{input:?}");
    assert_eq!(
        words(&runtime, &cell("k1", "stdout"))?,
        "hello from the kernel"
    );
    assert_eq!(words(&runtime, &cell("k1", "stderr"))?, "a warning");
    let markdown = words(&runtime, &cell("k2", "value"))?;
    assert!(
        markdown.contains("Bold") && !markdown.contains("**"),
        "rendered: {markdown:?}"
    );
    let frame = words(&runtime, &cell("k3", "value"))?;
    assert!(
        frame.contains("Lima  18.0"),
        "the DataFrame's own text: {frame:?}"
    );
    let error = words(&runtime, &cell("k6", "error"))?;
    assert!(
        error.starts_with("ValueError: boom"),
        "name and message first: {error:?}"
    );
    assert!(
        error.contains("raise ValueError('boom')"),
        "then the traceback: {error:?}"
    );
    Ok(())
}

/// The chart fits the pane, keeps its 3:1 aspect, and a press shows it at full size.
#[test]
fn a_picture_fits_the_pane_and_a_press_shows_it_full_size() -> Check {
    let (mut runtime, key) = notebook(TURN, usize::MAX)?;
    let picture = format!("{key}.cell.k4.picture.1");
    reveal(&mut runtime, &picture);
    let [_, _, w, h] = rect(&runtime, &picture)?;
    let [_, _, room, _] = rect(&runtime, &format!("{key}.cells"))?;
    assert!(w < 1200.0 && w <= room, "fitted: {w} in {room}");
    assert!((w / h - 3.0).abs() < 0.02, "its aspect: {w} by {h}");
    tap(&mut runtime, &picture)?;
    let [_, _, w, h] = rect(&runtime, &picture)?;
    assert_eq!((w, h), (1200.0, 400.0), "one point a pixel");
    tap(&mut runtime, &picture)?;
    let [_, _, w, _] = rect(&runtime, &picture)?;
    assert!(w < 1200.0, "fitted again: {w}");
    Ok(())
}

/// 3,000 printed lines are cut where the notice says, and the notice copies all of them.
#[test]
fn a_long_output_is_cut_with_a_notice_that_copies_the_rest() -> Check {
    let (mut runtime, key) = notebook(TURN, usize::MAX)?;
    let printed: String = (0..3000).map(|n| format!("line {n}\n")).collect();
    let printed = printed.trim_end();
    let total = printed.chars().count();
    let cut = format!("{key}.cell.k5.stdout.cut");
    let said = words(&runtime, &cut)?;
    assert!(
        said.starts_with(&format!("[…] 20000 of {total} characters")),
        "{said:?}"
    );
    let clipboard = rune::clipboard::memory();
    runtime.use_clipboard(&clipboard);
    tap(&mut runtime, &cut)?;
    assert_eq!(
        clipboard.text().map(|t| t.to_string()).as_deref(),
        Some(printed)
    );
    Ok(())
}

/// A cell's Copy takes its code whole, as the model wrote it.
#[test]
fn copy_on_a_cell_takes_its_code() -> Check {
    let (mut runtime, key) = notebook(TURN, usize::MAX)?;
    let clipboard = rune::clipboard::memory();
    runtime.use_clipboard(&clipboard);
    let copy = format!("{key}.cell.k3.copy");
    reveal(&mut runtime, &copy);
    tap(&mut runtime, &copy)?;
    let code = "import pandas as pd\npd.DataFrame({'city': ['Oslo', 'Lima', 'Pune'], 'temp': [4.5, 18.0, 31.2]})";
    assert_eq!(
        clipboard.text().map(|t| t.to_string()).as_deref(),
        Some(code)
    );
    Ok(())
}

/// A running cell says so, and its result takes its place when the server sends it.
#[test]
fn a_running_cell_says_so_until_its_outputs_arrive() -> Check {
    let (mut runtime, key) = notebook(TURN, THIRD_STARTED)?;
    let input = format!("{key}.cell.k3.input");
    assert!(
        words(&runtime, &input)?.starts_with("In [*]"),
        "{:?}",
        words(&runtime, &input)
    );
    assert_eq!(words(&runtime, &format!("{key}.cell.k3.state"))?, "Running");
    let rest_of_it = lines(TURN, THIRD_STARTED, THIRD_ENDED);
    runtime.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let _ = play(&mut hub.model, &rest_of_it);
        }
    });
    rest(&mut runtime);
    assert!(
        !live(&runtime, &format!("{key}.cell.k3.state")),
        "no longer running"
    );
    assert!(words(&runtime, &format!("{key}.cell.k3.value"))?.contains("Oslo"));
    Ok(())
}

/// The chat's ipython step opens the pane scrolled to that cell, not to the newest one.
#[test]
fn open_in_ipython_under_a_step_shows_that_cell() -> Check {
    let mut runtime = window(TURN, usize::MAX)?;
    let link = runtime.elements().into_iter().filter(|e| !e.exiting);
    let mut link = link
        .filter_map(|e| e.key)
        .filter(|key| key.ends_with(".ipython.k1"));
    let link = link
        .next()
        .ok_or("no Open in IPython under the first cell's step")?;
    assert_eq!(words(&runtime, &link)?, "Open in IPython");
    tap(&mut runtime, &link)?;
    let key = pane(&runtime)?;
    let [_, top, _, tall] = rect(&runtime, &format!("{key}.cells"))?;
    let [_, y, _, _] = rect(&runtime, &format!("{key}.cell.k1.input"))?;
    assert!(
        y >= top && y < top + tall,
        "the first cell in view: {y} in {top}..{}",
        top + tall
    );
    Ok(())
}

/// A thread whose agent never called ipython has no kernel yet, and the pane says both.
#[test]
fn a_thread_with_no_cells_says_its_kernel_is_not_booted() -> Check {
    let (runtime, key) = notebook(PLAIN, usize::MAX)?;
    let said = words(&runtime, &format!("{key}.state"))?;
    assert!(said.contains("No cells in this thread yet"), "{said:?}");
    assert!(said.contains("kernel is not booted"), "{said:?}");
    Ok(())
}
