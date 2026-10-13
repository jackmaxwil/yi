use ratatui::text::Line;
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;
use yi_console::app::notebook::apply_notebook;
use yi_console::model::NbCell;
use yi_console::panes::notebook_lines;
use yi_tui::colors::{ColorTier, Theme};
use yi_types::acp::{AcpSessionUpdate, AcpToolCallStatus, AcpToolCallUpdate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WIDTH: usize = 56;

fn rows(cells: &[NbCell], width: usize, full: bool) -> Vec<String> {
    let theme = Theme::new(ColorTier::TrueColor, true);
    notebook_lines(cells, width, full, &theme, "⌘W")
        .iter()
        .map(|line: &Line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn start(id: &str, code: Value) -> AcpSessionUpdate {
    AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
        tool_call_id: id.to_owned(),
        title: Some("ipython".to_owned()),
        raw_input: Some(code),
        ..AcpToolCallUpdate::default()
    })
}

fn end(id: &str, details: Value) -> AcpSessionUpdate {
    AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
        tool_call_id: id.to_owned(),
        status: Some(AcpToolCallStatus::Completed),
        raw_output: Some(details),
        ..AcpToolCallUpdate::default()
    })
}

// The scrubber masks call ids, so results pair with calls in order.
fn replay(session: &str) -> Result<Vec<NbCell>, serde_json::Error> {
    let (mut cells, mut calls, mut results) = (Vec::new(), 0_usize, 0_usize);
    for line in session.lines() {
        let entry: Value = serde_json::from_str(line)?;
        let message = entry.get("message").cloned().unwrap_or_default();
        let field = |key: &str| message.get(key).and_then(Value::as_str).unwrap_or_default();
        if field("role") == "assistant" {
            for call in message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = |key: &str| call.get(key).and_then(Value::as_str);
                if text("type") == Some("toolCall") && text("name") == Some("ipython") {
                    calls += 1;
                    let args = call.get("arguments").cloned().unwrap_or_default();
                    apply_notebook(&mut cells, &start(&format!("call-{calls}"), args));
                }
            }
        } else if field("role") == "toolResult" && field("toolName") == "ipython" {
            results += 1;
            let details = message.get("details").cloned().unwrap_or_default();
            apply_notebook(&mut cells, &end(&format!("call-{results}"), details));
        }
    }
    Ok(cells)
}

fn recorded() -> Result<Vec<NbCell>, serde_json::Error> {
    replay(include_str!(
        "../../tui/tests/fixtures/sessions/01a13357.jsonl"
    ))
}

#[test]
fn a_wrapped_code_row_hangs_deeper_than_its_line_and_says_so() -> TestResult {
    let rows = rows(&recorded()?, WIDTH, false);
    let head = rows
        .iter()
        .position(|row| row.contains("if ln.startswith(\"├\")"))
        .ok_or("no if row")?;
    let tail = rows.get(head + 1).ok_or("no continuation")?;
    assert!(tail.contains("startswith(\"│\"):"), "{tail:?}");
    assert!(tail.trim_start().starts_with('↪'), "{tail:?}");
    let indent = |row: &str, needle: &str| {
        row.find(needle)
            .and_then(|at| row.get(..at))
            .map(str::width)
    };
    assert!(
        indent(tail, "ln.startswith(\"│\")") > indent(&rows[head], "if ln"),
        "{:?} / {tail:?}",
        rows[head]
    );
    Ok(())
}

#[test]
fn a_line_too_long_for_a_row_starts_beside_its_number() -> TestResult {
    let rows = rows(&recorded()?, WIDTH, false);
    let bare = rows
        .iter()
        .find(|row| !row.trim().is_empty() && row.trim().chars().all(|c| c.is_ascii_digit()));
    assert_eq!(bare, None, "{rows:#?}");
    Ok(())
}

#[test]
fn recorded_stdout_breaks_between_words_never_inside_one() -> TestResult {
    let cells = recorded()?;
    let rows = rows(&cells, WIDTH, false);
    for cell in &cells {
        for word in cell
            .stdout
            .split_whitespace()
            .filter(|w| w.width() < WIDTH - 2)
        {
            assert!(
                rows.iter().any(|row| row.contains(word)),
                "{word:?} was split"
            );
        }
    }
    Ok(())
}

#[test]
fn a_recorded_traceback_keeps_its_last_frame_and_names_the_cut() -> TestResult {
    let cells = replay(include_str!(
        "../../tui/tests/fixtures/sessions/01a0648f.jsonl"
    ))?;
    let cell = cells
        .iter()
        .find(|cell| {
            cell.error
                .as_deref()
                .is_some_and(|e| e.contains("RuntimeError"))
        })
        .ok_or("no RuntimeError cell")?;
    let rows = rows(std::slice::from_ref(cell), 80, false);
    let last = cell
        .error
        .as_deref()
        .and_then(|e| e.lines().last())
        .ok_or("empty")?;
    assert!(
        rows.iter().any(|row| row.ends_with(last)),
        "{last:?} cut: {rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("-----------")),
        "{rows:#?}"
    );
    let notice = rows
        .iter()
        .find(|row| row.contains("[…] traceback"))
        .ok_or("silent cut")?;
    assert!(notice.contains("traceback last 12 of"), "{notice:?}");
    assert!(
        notice.contains("notebook cap 12 · v shows every row"),
        "{notice:?}"
    );
    Ok(())
}

fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("x{n}\n")).collect()
}

#[test]
fn every_cell_cap_speaks_one_row_past_its_limit() -> TestResult {
    type Fill = fn(&mut NbCell, String);
    let fields: [(&str, usize, bool, Fill); 5] = [
        ("code", 24, false, |cell, text| cell.code = text),
        ("stdout", 40, false, |cell, text| cell.stdout = text),
        ("stderr", 40, false, |cell, text| cell.stderr = text),
        ("result", 20, false, |cell, text| cell.result = Some(text)),
        ("traceback", 12, true, |cell, text| cell.error = Some(text)),
    ];
    for (noun, cap, tail, fill) in fields {
        let mut cell = NbCell {
            number: 1,
            ..NbCell::default()
        };
        fill(&mut cell, numbered(cap));
        let whole = rows(std::slice::from_ref(&cell), 80, false);
        assert!(
            !whole.iter().any(|row| row.contains("[…]")),
            "{noun} at cap: {whole:#?}"
        );
        assert!(
            whole.iter().any(|row| row.ends_with(&format!("x{cap}"))),
            "{noun}"
        );
        fill(&mut cell, numbered(cap + 1));
        let cut = rows(std::slice::from_ref(&cell), 80, false);
        let unit = if noun == "code" { "lines" } else { "rows" };
        let end = if tail { "last" } else { "first" };
        let said = format!("[…] {noun} {end} {cap} of {} {unit}", cap + 1);
        assert!(
            cut.iter().any(|row| row.contains(&said)),
            "{said:?}: {cut:#?}"
        );
        let gone = if tail {
            "x1".to_owned()
        } else {
            format!("x{}", cap + 1)
        };
        assert!(
            !cut.iter().any(|row| row.ends_with(&gone)),
            "{noun} kept {gone}"
        );
        let all = rows(std::slice::from_ref(&cell), 80, true);
        assert!(
            !all.iter().any(|row| row.contains("[…]")),
            "{noun} in full view"
        );
        assert!(
            all.iter().any(|row| row.ends_with(&gone)),
            "{noun} full lost {gone}"
        );
    }
    Ok(())
}

#[test]
fn dropped_cells_are_counted_and_survivors_keep_their_numbers() -> TestResult {
    let mut cells = Vec::new();
    for n in 1..=yi_console::model::KEPT_CELLS {
        apply_notebook(
            &mut cells,
            &start(&format!("c{n}"), json!({"code": "pass"})),
        );
    }
    let first = rows(&cells, 80, false);
    assert!(
        !first.iter().any(|row| row.contains("[…]")),
        "nothing dropped yet"
    );
    apply_notebook(&mut cells, &start("late", json!({"code": "pass"})));
    let numbers: Vec<usize> = cells.iter().map(|cell| cell.number).collect();
    assert_eq!(numbers.first(), Some(&2));
    assert_eq!(numbers.last(), Some(&(yi_console::model::KEPT_CELLS + 1)));
    let after = rows(&cells, 80, false);
    let notice = after.first().ok_or("empty")?;
    assert!(
        notice.contains("[…] 1 older cell not shown · notebook keeps 50"),
        "{notice:?}"
    );
    assert!(
        after.iter().any(|row| row.starts_with("◐ [2]")),
        "{after:#?}"
    );
    assert!(
        !after.iter().any(|row| row.starts_with("◐ [1]")),
        "renumbered"
    );
    Ok(())
}
