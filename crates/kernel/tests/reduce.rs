use serde_json::{Map, Value, json};
use yi_kernel::ATTACHMENT_DISPLAY_MIME;
use yi_kernel::reduce::{CellState, Reduction, finish, reduce};
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
