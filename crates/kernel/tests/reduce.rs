use serde_json::{Map, Value, json};
use yi_kernel::ATTACHMENT_DISPLAY_MIME;
use yi_kernel::reduce::{
    CellState, MAX_DISPLAY_CHARS, MAX_DISPLAY_IMAGES, Reduction, finish, reduce,
};
use yi_types::kernel::{ExecuteStatus, JupyterHeader, JupyterMessage};

fn message(msg_type: &str, parent: &str, content: Value) -> JupyterMessage {
    let mut parent_header = Map::new();
    parent_header.insert("msg_id".to_owned(), Value::String(parent.to_owned()));
    JupyterMessage {
        header: JupyterHeader {
            msg_id: "m".to_owned(),
            session: "s".to_owned(),
            username: "yi".to_owned(),
            date: "2026-08-24T00:00:00Z".to_owned(),
            msg_type: msg_type.to_owned(),
            version: "5.3".to_owned(),
        },
        parent_header,
        metadata: Map::new(),
        content: content.as_object().cloned().unwrap_or_default(),
    }
}

fn cell(max_chars: usize) -> CellState {
    CellState::new("req".to_owned(), max_chars, false)
}

fn stdout(cell: &mut CellState, text: &str) {
    let content = json!({"name": "stdout", "text": text});
    reduce(cell, &message("stream", "req", content), None);
}

fn fixture(name: &str) -> std::io::Result<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(dir.join("../tools/tests/fixtures/view").join(name))
}

fn cut_row(kept: usize, total: usize) -> String {
    format!(
        "[\u{2026} {kept} of {total} chars kept, the middle cut (DEFAULT_MAX_OUTPUT_CHARS {kept});"
    )
}

#[test]
fn last_line_image_and_html_reach_the_model_and_clients() {
    let mut cell = cell(1000);
    let data =
        json!({"image/png": "iVBORw0KGgo=", "text/html": "<b>table</b>", "text/plain": "Image"});
    reduce(
        &mut cell,
        &message("execute_result", "req", json!({"data": data})),
        None,
    );
    let result = finish(cell, 5, false);
    assert_eq!(
        result.attachments.len(),
        1,
        "the last line's image must reach the model"
    );
    assert_eq!(
        result.displays.len(),
        1,
        "the last line's html must reach clients"
    );
    assert_eq!(result.result.as_deref(), Some("Image"));
}

#[test]
fn stream_capture_caps_at_max_chars_but_ui_sees_everything() {
    let mut cell = cell(10);
    let mut ui = String::new();
    let mut sink = |chunk: yi_kernel::reduce::StreamChunk<'_>| ui.push_str(chunk.text);
    for _ in 0..3 {
        let step = reduce(
            &mut cell,
            &message(
                "stream",
                "req",
                json!({"name": "stdout", "text": "abcdefgh"}),
            ),
            Some(&mut sink),
        );
        assert_eq!(step, Reduction::Continue);
    }
    assert_eq!(ui.len(), 24, "on_stream must see uncapped chunks");
    let result = finish(cell, 5, false);
    let lines: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(lines.first(), Some(&"abcde"), "{}", result.stdout);
    assert_eq!(lines.last(), Some(&"defgh"), "{}", result.stdout);
    assert!(
        lines.iter().any(|line| line.starts_with(&cut_row(10, 24))),
        "the model-facing capture must carry the cut row: {}",
        result.stdout
    );
}

/// A failure printed last in a long cell sits in its tail, which a head-only cap dropped.
#[test]
fn a_stream_at_the_cap_passes_whole_and_one_past_keeps_both_ends() -> std::io::Result<()> {
    let log = fixture("cargo-test-3000-cell-spill.txt")?;
    for source in [log, "\u{e9}".repeat(70_000)] {
        let at: String = source.chars().take(65_536).collect();
        let mut capture = cell(65_536);
        stdout(&mut capture, &at);
        assert!(
            finish(capture, 1, false).stdout == at,
            "65,536 chars pass whole"
        );

        let past: String = source.chars().take(65_537).collect();
        let mut capture = cell(65_536);
        stdout(&mut capture, &past);
        let shown = finish(capture, 1, false).stdout;
        let head: String = past.chars().take(32_768).collect();
        let tail: String = past.chars().skip(32_769).collect();
        let row = cut_row(65_536, 65_537);
        assert!(
            shown.starts_with(&format!("{head}\n{row}")),
            "head or row lost"
        );
        assert!(shown.ends_with(&format!("]\n{tail}")), "tail lost");
    }
    Ok(())
}

/// The cap's middle trim runs as chunks arrive and still keeps the first and last halves.
#[test]
fn a_stream_many_times_the_cap_keeps_its_first_and_last_halves() -> std::io::Result<()> {
    let whole = fixture("cargo-test-3000-cell-spill.txt")?.repeat(6);
    let mut capture = cell(65_536);
    let chars: Vec<char> = whole.chars().collect();
    for chunk in chars.chunks(4_096) {
        stdout(&mut capture, &chunk.iter().collect::<String>());
        assert!(
            capture.stdout.len() <= 8 * 65_536 + 4 * 4_096,
            "the capture outgrew its cap"
        );
    }
    let shown = finish(capture, 1, false).stdout;
    let head: String = whole.chars().take(32_768).collect();
    let tail: String = whole
        .chars()
        .skip(chars.len().saturating_sub(32_768))
        .collect();
    let row = cut_row(65_536, chars.len());
    assert!(
        shown.starts_with(&format!("{head}\n{row}")),
        "head or row lost"
    );
    assert!(shown.ends_with(&format!("]\n{tail}")), "tail lost");
    Ok(())
}

/// A chunk that lands exactly on the cap left the capture full, and every later chunk was
/// dropped with no row.
#[test]
fn a_chunk_ending_on_the_cap_then_more_text_is_cut_with_its_row() -> std::io::Result<()> {
    let log = fixture("cargo-test-3000-cell-spill.txt")?;
    let last = log.lines().last().unwrap_or_default();
    let mut capture = cell(65_536);
    stdout(&mut capture, &log.chars().take(65_536).collect::<String>());
    stdout(&mut capture, last);
    let shown = finish(capture, 1, false).stdout;
    let row = cut_row(65_536, 65_536 + last.chars().count());
    assert!(shown.contains(&row), "no cut row");
    assert!(shown.ends_with(last), "the late chunk was dropped");
    Ok(())
}

/// What the model printed on purpose, under the cap, arrives byte for byte.
#[test]
fn a_thirty_kilobyte_print_is_byte_identical() -> std::io::Result<()> {
    let printed = fixture("bash-cell-print.txt")?;
    let mut capture = cell(65_536);
    for line in printed.split_inclusive('\n') {
        stdout(&mut capture, line);
    }
    assert!(finish(capture, 1, false).stdout == printed);
    Ok(())
}

#[test]
fn oversized_attachment_fails_the_cell_instead_of_silently_dropping() {
    let mut cell = cell(65_536);
    let big = "x".repeat(10_000_001);
    let content =
        json!({"data": {ATTACHMENT_DISPLAY_MIME: {"mime_type": "image/png", "data": big}}});
    reduce(&mut cell, &message("display_data", "req", content), None);
    assert_eq!(cell.status, ExecuteStatus::Error);
    assert!(cell.stderr.contains("attachment dropped"));
    assert!(cell.attachments.is_empty());
}

/// An attachment's data is written into a kitty escape and sent to the provider as is, so
/// only strict base64 whose head carries its type's magic number is kept (#817). The images
/// are Pillow's 1x1 encodes, the PNG's carrying `+` and `/`, the JPEG cut to twelve bytes.
#[test]
fn an_attachment_is_kept_only_as_base64_of_the_image_it_names() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";
    let gif = "R0lGODdhAQABAIEAAMgeHgAAAAAAAAAAACwAAAAAAQABAAAIBAABBAQAOw==";
    let webp =
        "UklGRjoAAABXRUJQVlA4IC4AAACwAQCdASoBAAEAAUAmJaACdLoABDAAAP7x3I/4DdfFtMv/vYL/3YL/3YL/WwAA";
    let jpeg = "/9j/4AAQSkZJRgAB";
    let cases = [
        ("image/png", png, true),
        ("image/gif", gif, true),
        ("image/webp", webp, true),
        ("image/jpeg", jpeg, true),
        ("image/bmp", "QUJD", true),
        (
            "image/png",
            "AAAA\u{1b}\\\u{1b}]52;c;ZWNobyBwd25lZA==\u{7}",
            false,
        ),
        ("image/png", jpeg, false),
        ("image/png", &png[..png.len() - 1], false),
        ("image/png", &format!("{png}=AAA"), false),
    ];
    for (mime_type, data, kept) in cases {
        let mut cell = cell(65_536);
        let content =
            json!({"data": {ATTACHMENT_DISPLAY_MIME: {"mime_type": mime_type, "data": data}}});
        reduce(&mut cell, &message("display_data", "req", content), None);
        assert_eq!(
            cell.attachments.len(),
            usize::from(kept),
            "{mime_type} {data:?}"
        );
        assert_eq!(
            cell.stderr.contains("attachment dropped"),
            !kept,
            "{mime_type} {data:?}: {}",
            cell.stderr
        );
    }
}

#[test]
fn error_result_and_idle_reduce_into_the_cell() {
    let mut cell = cell(65_536);
    reduce(
        &mut cell,
        &message(
            "execute_result",
            "req",
            json!({"data": {"text/plain": "42"}}),
        ),
        None,
    );
    assert_eq!(cell.result.as_deref(), Some("42"));

    reduce(
        &mut cell,
        &message(
            "error",
            "req",
            json!({"ename": "ValueError", "evalue": "boom", "traceback": ["t1"]}),
        ),
        None,
    );
    assert_eq!(cell.status, ExecuteStatus::Error);

    let busy = reduce(
        &mut cell,
        &message("status", "req", json!({"execution_state": "busy"})),
        None,
    );
    assert_eq!(busy, Reduction::Continue);
    let idle = reduce(
        &mut cell,
        &message("status", "req", json!({"execution_state": "idle"})),
        None,
    );
    assert_eq!(
        idle,
        Reduction::Done,
        "only a matching idle settles the cell"
    );

    let result = finish(cell, 7, false);
    assert_eq!(result.status, ExecuteStatus::Error);
    assert_eq!(
        result.error.map(|error| error.ename),
        Some("ValueError".to_owned())
    );
}

/// A failure written last to stderr, and the last items of a long repr, sat in the half a
/// head-only cap dropped; each keeps both ends around its row, the cut char gone.
#[test]
fn stderr_and_a_displayed_value_one_past_the_cap_keep_both_ends() -> std::io::Result<()> {
    let past = |text: String| -> String { text.chars().take(65_537).collect() };
    let log = past(fixture("cargo-test-3000-cell-spill.txt")?);
    let repr = past(fixture("list-repr-20000.txt")?);
    let cases = [
        ("stream", json!({"name": "stderr", "text": log}), &log),
        (
            "execute_result",
            json!({"data": {"text/plain": repr}}),
            &repr,
        ),
    ];
    for (kind, content, source) in cases {
        let mut capture = cell(65_536);
        reduce(&mut capture, &message(kind, "req", content), None);
        let done = finish(capture, 1, false);
        let shown = done.result.unwrap_or(done.stderr);
        let head: String = source.chars().take(32_768).collect();
        let tail: String = source.chars().skip(32_769).collect();
        let across: String = source.chars().skip(32_760).take(16).collect();
        let row = cut_row(65_536, 65_537);
        assert!(
            shown.starts_with(&format!("{head}\n{row}")),
            "{kind}: head or row lost"
        );
        assert!(shown.ends_with(&format!("]\n{tail}")), "{kind}: tail lost");
        assert!(!shown.contains(&across), "{kind}: the middle was kept");
    }
    Ok(())
}

/// Dies with a value whose markdown or JSON bundle was thrown away for its text/plain repr: the
/// notebook has nothing but the flat form to draw.
#[test]
fn a_displayed_value_keeps_the_richest_bundle_its_data_names() {
    let cases = [
        (
            json!({"text/plain": "{'a': 1}", "application/json": {"a": 1}, "text/markdown": "# Tide"}),
            "# Tide",
            Some("text/markdown"),
        ),
        (
            json!({"text/plain": "{'a': 1}", "application/json": {"a": 1}}),
            "{\"a\":1}",
            Some("application/json"),
        ),
        (
            json!({"text/plain": "hello", "application/json": "hello"}),
            "\"hello\"",
            Some("application/json"),
        ),
        (json!({"text/plain": "42"}), "42", Some("text/plain")),
    ];
    for (data, text, mime) in cases {
        let mut cell = cell(65_536);
        reduce(
            &mut cell,
            &message("execute_result", "req", json!({"data": data})),
            None,
        );
        assert_eq!(cell.result.as_deref(), Some(text), "{data}");
        assert_eq!(cell.result_mime.as_deref(), mime, "{data}");
        assert_eq!(
            finish(cell, 1, false).result_mime.as_deref(),
            mime,
            "{data} lost its mime on the way out"
        );
    }
    let mut empty = cell(65_536);
    reduce(
        &mut empty,
        &message("execute_result", "req", json!({"data": {}})),
        None,
    );
    assert_eq!(empty.result, None);
    assert_eq!(empty.result_mime, None);
}

/// Dies with a bundle member past the cell's char cap stored whole: a JSON document of a
/// million characters would ride every session line, and one beside a text/plain repr had a
/// drawable form to fall back on.
#[test]
fn a_bundle_past_the_cells_cap_falls_back_to_its_plain_text() {
    let big = json!({"rows": "x".repeat(600)});
    let mut paired = cell(256);
    reduce(
        &mut paired,
        &message(
            "execute_result",
            "req",
            json!({"data": {"text/plain": "plain", "application/json": big}}),
        ),
        None,
    );
    assert_eq!(paired.result.as_deref(), Some("plain"));
    assert_eq!(paired.result_mime.as_deref(), Some("text/plain"));

    let mut solo = cell(256);
    reduce(
        &mut solo,
        &message(
            "execute_result",
            "req",
            json!({"data": {"application/json": big}}),
        ),
        None,
    );
    let kept = finish(solo, 1, false);
    let shown = kept.result.unwrap_or_default();
    assert!(shown.contains("DEFAULT_MAX_OUTPUT_CHARS 256"), "{shown}");
    assert_eq!(kept.result_mime.as_deref(), Some("application/json"));
}

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";

fn display(cell: &mut CellState, kind: &str, data: Value) {
    reduce(cell, &message(kind, "req", json!({ "data": data })), None);
}

/// Incident (#1915): `display(Image(...))` and an HTML table returned only the cell's prints.
/// Jupyter wraps the PNG's base64 in a trailing newline, which strict base64 would refuse.
#[test]
fn a_displayed_png_reaches_the_model_and_an_invalid_one_says_so() {
    let mut cell = cell(65_536);
    let bundle = json!({"image/png": format!("{PNG}\n"), "text/plain": "<IPython.core.display.Image object>"});
    display(&mut cell, "display_data", bundle);
    display(
        &mut cell,
        "display_data",
        json!({"image/png": "bm90LWEtcG5n", "text/plain": "<Image>"}),
    );
    assert_eq!(cell.attachments.len(), 1);
    assert_eq!(cell.attachments[0].data, PNG);
    let done = finish(cell, 1, false);
    assert_eq!(done.status, ExecuteStatus::Ok);
    assert!(!done.stdout.contains("IPython.core"), "{}", done.stdout);
    assert!(
        done.notices.len() == 1 && done.notices[0].starts_with("[image/png display not shown: "),
        "{:?}",
        done.notices
    );
}

/// An HTML display with a `text/plain` fallback reads as that fallback and stays whole for a
/// client; one with none is named in the cell's one notice, counted per type.
#[test]
fn an_html_display_reads_as_its_fallback_and_stays_for_clients() {
    let mut cell = cell(65_536);
    display(
        &mut cell,
        "display_data",
        json!({"text/html": "<table>é</table>", "text/plain": "   a\n0  1"}),
    );
    display(&mut cell, "display_data", json!({"text/html": "<b>x</b>"}));
    display(
        &mut cell,
        "display_data",
        json!({"application/javascript": "1", "text/plain": "t"}),
    );
    display(
        &mut cell,
        "update_display_data",
        json!({"text/plain": "progress 2"}),
    );
    let done = finish(cell, 1, false);
    assert!(done.stdout.contains("   a\n0  1\n"), "{}", done.stdout);
    assert!(!done.stdout.contains("progress 2"), "{}", done.stdout);
    let kept: Vec<_> = done
        .displays
        .iter()
        .map(|d| (d.mime_type.as_str(), d.data.as_str()))
        .collect();
    assert_eq!(
        kept,
        [("text/html", "<table>é</table>"), ("text/html", "<b>x</b>")]
    );
    assert_eq!(done.notices.len(), 1, "{:?}", done.notices);
    assert!(
        done.notices[0].contains("application/javascript x1"),
        "{:?}",
        done.notices
    );
}

/// Every cap speaks, at the limit and one past it.
#[test]
fn display_caps_name_what_was_kept_and_the_cap() {
    let mut cell = cell(65_536);
    for _ in 0..=MAX_DISPLAY_IMAGES {
        display(&mut cell, "display_data", json!({"image/png": PNG}));
    }
    assert_eq!(cell.attachments.len(), MAX_DISPLAY_IMAGES);
    let at = "é".repeat(MAX_DISPLAY_CHARS);
    display(&mut cell, "display_data", json!({"text/html": at}));
    assert_eq!(cell.displays.len(), 1);
    display(&mut cell, "display_data", json!({"text/html": "x"}));
    let done = finish(cell, 1, false);
    assert_eq!(done.displays.len(), 1);
    let rows = done.notices.join("\n");
    assert!(rows.contains(&format!("[… {MAX_DISPLAY_IMAGES} of 9 image displays shown (MAX_DISPLAY_IMAGES {MAX_DISPLAY_IMAGES})")), "{rows}");
    assert!(rows.contains(&format!("1 text/html or image/svg+xml displays not kept for clients (MAX_DISPLAY_CHARS {MAX_DISPLAY_CHARS} per cell)")), "{rows}");
}
