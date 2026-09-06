use std::error::Error;

use ratatui::text::Line;
use serde_json::json;
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::pycell::{head, preview, split_traceback};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn cell(details: serde_json::Value, status: ToolStatus) -> ToolCell {
    ToolCell {
        name: "ipython".to_owned(),
        call_id: String::new(),
        intent: None,
        status,
        summary: ToolCell::summary_of("ipython", ""),
        digest: None,
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details,
    }
}

/// The first line of a cell is usually an import or a comment. Naming it is how
/// every kernel call ends up looking identical in the transcript.
#[test]
fn the_preview_names_what_the_cell_did_not_its_first_line() -> TestResult {
    let code = "# load the frame\nimport pandas as pd\nfrom pathlib import Path\n\ndf = pd.read_csv(\"data.csv\")\nprint(df.head())";
    assert_eq!(preview(code), "df = pd.read_csv(\"data.csv\")");

    // An effect outranks a binding even when the binding comes first.
    let effects = "rows = compute(3)\nPath(\"out.txt\").write_text(rows)";
    assert_eq!(preview(effects), "Path(\"out.txt\").write_text(rows)");

    // A cell with nothing but bookkeeping still has to render.
    assert_eq!(preview("import os\n# nothing else"), "");
    Ok(())
}

/// A cell's source reaches the screen and every frame dump taken of it. A
/// credential pasted into a scratch cell must not survive that trip.
#[test]
fn the_preview_redacts_credentials_and_blobs() -> TestResult {
    assert!(!preview("api_key = \"abcd1234efgh5678\"").contains("abcd1234"));
    assert!(preview("api_key = \"abcd1234efgh5678\"").contains("<redacted>"));
    let call = "client = Anthropic(api_key=\"sk-ant1234567890abcdef\")";
    assert!(preview(call).contains("<redacted>"), "{}", preview(call));
    assert!(
        !preview(call).contains("ant1234567890"),
        "{}",
        preview(call)
    );

    // A hyphenated word containing the prefix is not a key.
    let ordinary = "queue = build_task_queue(\"task-oriented\")";
    assert!(
        !preview(ordinary).contains("<redacted>"),
        "{}",
        preview(ordinary)
    );

    let blob = format!("payload = \"{}\"", "QUJDREVG".repeat(6));
    assert!(preview(&blob).contains("<blob>"), "{}", preview(&blob));

    // A long ordinary identifier is not a blob: the run has to look encoded.
    let plain = "result = compute_the_quarterly_revenue_projection(2026)";
    assert!(!preview(plain).contains("<blob>"), "{}", preview(plain));
    Ok(())
}

/// Found by rendering a real frame: the head was redacted and the expanded
/// source was not, so one keystroke put the key back on screen. The whole
/// transcript is what gets shared, so both are redacted on the same terms.
#[test]
fn the_expanded_source_is_redacted_too() -> TestResult {
    let cell = cell(
        json!({ "code": "train(df, api_key=\"sk-live1234567890abcd\")" }),
        ToolStatus::Done,
    );
    for mode in [TranscriptMode::Normal, TranscriptMode::Verbose] {
        let joined = text(&Cell::Tool(cell.clone()).lines(120, &theme(), mode, 0)).join("\n");
        assert!(
            !joined.contains("sk-live1234567890abcd"),
            "the key reached the screen in {mode:?}: {joined}"
        );
        assert!(joined.contains("<redacted>"), "{joined}");
    }
    // The caller's own syntax survives redaction: an argument still reads as one.
    let head = head(&cell, 0);
    assert!(head.contains("train(df, <redacted>)"), "{head}");
    Ok(())
}

/// Invariant: a head that changes width when the body opens moves every row
/// under it, and the reader loses their place.
#[test]
fn the_head_line_is_byte_identical_in_every_mode() -> TestResult {
    let cell = cell(
        json!({
            "code": "df = pd.read_csv(\"data.csv\")\ndf.describe()",
            "stdout": "count 4\nmean 2\n",
            "durationMs": 340,
        }),
        ToolStatus::Done,
    );
    let expected = "⊙ python · df = pd.read_csv(\"data.csv\")";
    assert_eq!(head(&cell, 0), expected);
    let first = |mode| {
        text(&Cell::Tool(cell.clone()).lines(120, &theme(), mode, 0))
            .first()
            .cloned()
            .unwrap_or_default()
    };
    let normal = first(TranscriptMode::Normal);
    assert!(normal.contains("⊙ python  df = pd.read_csv"), "{normal}");
    assert!(normal.ends_with("↑ 2 ↓ 2 lines  340ms"), "{normal}");
    for mode in [TranscriptMode::Thinking, TranscriptMode::Verbose] {
        assert_eq!(first(mode), normal, "{mode:?}");
    }
    Ok(())
}

/// A `%%bash` cell's output is a shell's. Calling it python because the kernel
/// is a Python one tells the reader the wrong thing about the code.
#[test]
fn a_bash_cell_says_bash() -> TestResult {
    let cell = cell(json!({ "code": "%%bash\nls -la" }), ToolStatus::Done);
    assert!(head(&cell, 0).contains("⊙ bash ·"), "{}", head(&cell, 0));
    Ok(())
}

/// A reader who cannot see why the kernel raised cannot act on it, whatever
/// mode the cell rendered under.
#[test]
fn a_failed_cell_shows_its_source_and_traceback_in_normal_mode() -> TestResult {
    let cell = cell(
        json!({
            "code": "train(model)",
            "error": {
                "ename": "ValueError",
                "evalue": "shape mismatch",
                "traceback": ["Traceback (most recent call last):", "ValueError: shape mismatch"],
            },
            "durationMs": 4100,
        }),
        ToolStatus::Failed,
    );
    let rendered = text(&Cell::Tool(cell).lines(120, &theme(), TranscriptMode::Normal, 0));
    let joined = rendered.join("\n");
    assert!(joined.contains("ValueError"), "{joined}");
    assert!(joined.contains("› train(model)"), "{joined}");
    assert!(joined.contains("shape mismatch"), "{joined}");
    Ok(())
}

/// Without the split, a cell that printed before it raised reads as though the
/// print was part of the traceback.
#[test]
fn stdout_before_a_traceback_stays_stdout() -> TestResult {
    let (out, trace) =
        split_traceback("loading rows\nTraceback (most recent call last):\n  File x");
    assert_eq!(out, "loading rows\n");
    assert!(trace.starts_with("Traceback"));

    let (plain, none) = split_traceback("all fine\n");
    assert_eq!(plain, "all fine\n");
    assert!(none.is_empty());
    Ok(())
}

/// A cell that wrote three files earns the same rows an `edit` call would. The
/// patch arrives already computed, so this renders it rather than building one.
#[test]
fn kernel_side_file_edits_render_as_diffs() -> TestResult {
    let cell = cell(
        json!({
            "code": "patch()",
            "diffs": [{
                "path": "notes.txt",
                "patch": "--- a/notes.txt\n+++ b/notes.txt\n@@ -1,1 +1,1 @@\n-one\n+two\n",
            }],
        }),
        ToolStatus::Done,
    );
    let rendered = text(&Cell::Tool(cell).lines(120, &theme(), TranscriptMode::Normal, 0));
    let joined = rendered.join("\n");
    assert!(joined.contains("╰─ notes.txt"), "{joined}");
    assert!(
        joined.contains("│ one") && joined.contains("│ two"),
        "{joined}"
    );
    assert!(
        rendered
            .iter()
            .any(|row| row.trim_start_matches(['│', ' ']).starts_with('-'))
            && rendered
                .iter()
                .any(|row| row.trim_start_matches(['│', ' ']).starts_with('+')),
        "{joined}"
    );
    Ok(())
}

/// Collapsed, a kernel call is one row: the transcript is a conversation, not a
/// notebook, and forty lines of scratch output buries the answer under it.
#[test]
fn a_quiet_cell_collapses_to_one_row() -> TestResult {
    let cell = cell(
        json!({ "code": "x = 1", "stdout": "a\nb\nc\nd\ne\nf\n", "durationMs": 12 }),
        ToolStatus::Done,
    );
    let normal = text(&Cell::Tool(cell.clone()).lines(120, &theme(), TranscriptMode::Normal, 0));
    assert_eq!(normal.len(), 1, "{normal:?}");
    let verbose = text(&Cell::Tool(cell).lines(120, &theme(), TranscriptMode::Verbose, 0));
    assert!(verbose.len() > 6, "{verbose:?}");
    assert!(
        verbose.iter().any(|row| row.contains("› x = 1")),
        "{verbose:?}"
    );
    Ok(())
}

/// The screenshot's third defect: a 49-row asyncio traceback filled the screen
/// in every mode, and the head counted none of it. The head keeps the line that
/// raised, the tail keeps why, and the count says how much sits between.
#[test]
fn a_loud_failed_cell_is_bounded_and_its_lines_counted() -> TestResult {
    let mut traceback = vec![
        "Traceback (most recent call last):".to_owned(),
        "Cell In[2], line 2".to_owned(),
        "      1 h = await rlm.run('x')".to_owned(),
        "----> 2 r = await h.result(30)".to_owned(),
    ];
    for frame in 0..43 {
        traceback.push(format!(
            "  File \"/usr/lib/python3.13/asyncio/tasks.py\", line {}, in __step_run_and_handle_result",
            300 + frame
        ));
    }
    traceback.push("RuntimeError: child \"sub-1\" did not answer with JSON:".to_owned());
    traceback.push("RLM OK".to_owned());
    assert_eq!(traceback.len(), 49);
    let cell = cell(
        json!({
            "code": "h = await rlm.run('x')\nr = await h.result(30)\nprint(r)",
            "error": {
                "ename": "RuntimeError",
                "evalue": "child \"sub-1\" did not answer with JSON:\nRLM OK",
                "traceback": traceback,
            },
            "durationMs": 3579,
        }),
        ToolStatus::Failed,
    );
    let normal = text(&Cell::Tool(cell.clone()).lines(120, &theme(), TranscriptMode::Normal, 0));
    assert!(normal.len() <= 16, "{} rows: {normal:?}", normal.len());
    assert!(
        normal.iter().any(|row| row.contains("… 39 more lines")),
        "{normal:?}"
    );
    assert!(
        normal
            .iter()
            .any(|row| row.contains("----> 2 r = await h.result(30)")),
        "the head keeps the line that raised: {normal:?}"
    );
    let tail = normal.len().saturating_sub(5);
    assert!(
        normal
            .iter()
            .skip(tail)
            .any(|row| row.contains("RuntimeError: child")),
        "the tail keeps why: {normal:?}"
    );
    assert!(
        normal.last().is_some_and(|row| row.ends_with("RLM OK")),
        "{normal:?}"
    );
    assert!(
        normal.first().is_some_and(|row| row.contains("↓ 49 lines")),
        "the count includes the traceback: {normal:?}"
    );
    let verbose = text(&Cell::Tool(cell).lines(120, &theme(), TranscriptMode::Verbose, 0));
    assert!(verbose.len() > 50, "verbose stays whole: {}", verbose.len());
    Ok(())
}
