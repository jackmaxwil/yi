use serde_json::{Map, Value, json};
use yi_kernel::reduce::{CellState, Reduction, finish, reduce};
use yi_kernel::{ATTACHMENT_DISPLAY_MIME, DIFF_DISPLAY_MIME};
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
    CellState::new("req".to_owned(), "code".to_owned(), max_chars, false)
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
    assert!(result.stdout.starts_with("abcdefghab"));
    assert!(
        result
            .stdout
            .contains("[... output truncated at 10 chars ...]"),
        "the model-facing capture must carry the truncation marker"
    );
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
/// only strict base64 of the image its mime_type names is kept (#817). The images are
/// Pillow's 1x1 encodes, the JPEG cut to its first twelve bytes.
#[test]
fn an_attachment_is_kept_only_as_base64_of_the_image_it_names() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4IScHAAK2AQU0pnWqAAAAAElFTkSuQmCC";
    let gif = "R0lGODdhAQABAIEAAMgeHgAAAAAAAAAAACwAAAAAAQABAAAIBAABBAQAOw==";
    let webp =
        "UklGRjoAAABXRUJQVlA4IC4AAACwAQCdASoBAAEAAUAmJaACdLoABDAAAP7x3I/4DdfFtMv/vYL/3YL/3YL/WwAA";
    let jpeg = "/9j/4AAQSkZJRgAB";
    let cases = [
        ("image/png", png, true),
        ("image/gif", gif, true),
        ("image/webp", webp, true),
        ("image/jpeg", jpeg, true),
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
fn diff_error_result_and_idle_reduce_into_the_cell() {
    let mut cell = cell(65_536);
    let diff = json!({"data": {DIFF_DISPLAY_MIME: {"path": "/a.txt", "old_str": "a", "new_str": "b", "start_line": 3}}});
    reduce(&mut cell, &message("display_data", "req", diff), None);
    assert_eq!(cell.diffs.len(), 1);
    assert_eq!(cell.diffs[0].start_line, Some(3));

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
