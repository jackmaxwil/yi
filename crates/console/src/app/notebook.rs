use serde_json::Value;
use yi_types::acp::{AcpContentBlock, AcpSessionUpdate, AcpToolCallUpdate, AcpToolContent};

const TEXT_CAP: usize = 8 * 1024;
const RESULT_CAP: usize = 2 * 1024;
use crate::model::{IMAGE_BYTES_MAX, IMAGES_PER_CELL};

fn clamp_bytes(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let head = text
        .get(..text.floor_char_boundary(cap))
        .unwrap_or_default();
    format!(
        "{head}\n[… {} of {} bytes kept · notebook cap {} KiB · the transcript holds all of it]",
        head.len(),
        text.len(),
        cap / 1024
    )
}

/// Kernel cells out of the tool-call stream: the start carries the code,
/// the end carries streams and attachments in the details record.
pub(super) fn apply_notebook(
    cells: &mut Vec<crate::model::NbCell>,
    update: &AcpSessionUpdate,
) -> bool {
    let AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
        tool_call_id,
        title,
        raw_input,
        raw_output,
        status,
        content,
        ..
    }) = update
    else {
        return false;
    };
    let is_start = title.as_deref() == Some("ipython");
    let existing = cells.iter_mut().find(|cell| cell.call_id == *tool_call_id);
    match (existing, is_start) {
        (None, true) => {
            let code = raw_input
                .as_ref()
                .and_then(|input| input.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let number = cells.last().map_or(1, |cell| cell.number.saturating_add(1));
            cells.push(crate::model::NbCell {
                call_id: tool_call_id.clone(),
                number,
                code,
                running: true,
                ..crate::model::NbCell::default()
            });
            if cells.len() > crate::model::KEPT_CELLS {
                cells.remove(0);
            }
            true
        }
        (Some(cell), _) => {
            if let Some(details) = raw_output {
                cell.stdout = details
                    .get("stdout")
                    .and_then(Value::as_str)
                    .map(|text| clamp_bytes(text, TEXT_CAP))
                    .unwrap_or_default();
                cell.result = details
                    .get("result")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| clamp_bytes(text, RESULT_CAP));
                cell.result_mime = details
                    .get("resultMime")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                cell.stderr = details
                    .get("stderr")
                    .and_then(Value::as_str)
                    .map(|text| clamp_bytes(text, TEXT_CAP))
                    .unwrap_or_default();
                cell.duration_ms = details.get("durationMs").and_then(Value::as_u64);
                cell.error =
                    Some(yi_tui::pycell::traceback_of(details)).filter(|text| !text.is_empty());
                // Sessions from before the image moved into the result content keep
                // its bytes in the details record.
                let legacy = details
                    .get("attachmentMedia")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|item| {
                        item.get("mime_type").and_then(Value::as_str) == Some("image/png")
                    })
                    .filter_map(|item| item.get("data"));
                let current = content.iter().flatten().filter_map(|block| match block {
                    AcpToolContent::Content {
                        content: AcpContentBlock::Other(block),
                    } if block.block_type == "image"
                        && block.fields.get("mimeType").and_then(Value::as_str)
                            == Some("image/png") =>
                    {
                        block.fields.get("data")
                    }
                    _ => None,
                });
                let all: Vec<&str> = legacy.chain(current).filter_map(Value::as_str).collect();
                cell.dropped_image_bytes = all
                    .iter()
                    .filter(|data| data.len() > IMAGE_BYTES_MAX)
                    .map(|data| data.len())
                    .collect();
                let kept: Vec<&str> = all
                    .into_iter()
                    .filter(|data| data.len() <= IMAGE_BYTES_MAX)
                    .collect();
                cell.images_past_limit = kept.len().saturating_sub(IMAGES_PER_CELL);
                cell.images = kept
                    .into_iter()
                    .skip(cell.images_past_limit)
                    .map(str::to_owned)
                    .collect();
                cell.image_bytes = cell.images.iter().map(String::len).collect();
            }
            if status.is_some() || raw_output.is_some() {
                cell.running = false;
            }
            if !cell.images.is_empty() {
                let older = cells
                    .iter_mut()
                    .filter(|cell| cell.call_id != *tool_call_id);
                older.for_each(|cell| cell.images = Vec::new());
            }
            true
        }
        (None, false) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::apply_notebook;
    use crate::model::NbCell;
    use crate::panes::notebook_lines;
    use ratatui::text::Line;
    use serde_json::{Value, json};
    use unicode_width::UnicodeWidthStr;
    use yi_tui::colors::{ColorTier, Theme};
    use yi_tui::highlight::Token;
    use yi_types::acp::{AcpSessionUpdate, AcpToolCallStatus, AcpToolCallUpdate};
    use yi_types::message::{AgentMessage, Content};

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
    mod session {
        include!("../../../tui/tests/common/session.rs");
    }

    fn replay(session: &str) -> Result<Vec<NbCell>, serde_json::Error> {
        let (mut cells, mut calls, mut results) = (Vec::new(), 0_usize, 0_usize);
        for message in session::session_entries(session)? {
            match &message {
                AgentMessage::Assistant { content, .. } => {
                    for call in content {
                        if let Content::ToolCall {
                            name, arguments, ..
                        } = call
                            && name == "ipython"
                        {
                            calls += 1;
                            apply_notebook(
                                &mut cells,
                                &start(&format!("call-{calls}"), Value::Object(arguments.clone())),
                            );
                        }
                    }
                }
                AgentMessage::ToolResult {
                    tool_name, details, ..
                } => {
                    if tool_name == "ipython" {
                        results += 1;
                        apply_notebook(
                            &mut cells,
                            &end(
                                &format!("call-{results}"),
                                details.clone().unwrap_or_default(),
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(cells)
    }

    const RECORDED: &str = include_str!("../../../tui/tests/fixtures/sessions/01a13357.jsonl");

    fn recorded() -> Result<Vec<NbCell>, serde_json::Error> {
        replay(RECORDED)
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
            "../../../tui/tests/fixtures/sessions/01a0648f.jsonl"
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

    #[test]
    fn dropped_cells_are_counted_and_survivors_keep_their_numbers() -> TestResult {
        let mut cells = Vec::new();
        for n in 1..=crate::model::KEPT_CELLS {
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
        assert_eq!(numbers.last(), Some(&(crate::model::KEPT_CELLS + 1)));
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

    #[test]
    fn recorded_stderr_reaches_the_pane_beside_stdout() -> TestResult {
        let cells = replay(include_str!(
            "../../../tui/tests/fixtures/sessions/01a133ca.jsonl"
        ))?;
        let rows = rows(&cells, WIDTH, false);
        assert!(
            rows.iter().any(|row| row.trim() == "6 crates scanned"),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.trim() == "scanned kernel: no cycles"),
            "{rows:#?}"
        );
        Ok(())
    }

    #[test]
    fn a_cell_that_lands_after_a_draw_reaches_the_pane() -> TestResult {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = crate::app::App::new("/repo".to_owned(), theme);
        let session = crate::model::SessionId("s-nb".to_owned());
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = crate::model::PaneContent::Notebook {
                session: Some(session.clone()),
                full: false,
                drawn: None,
            };
        }
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let screen = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| -> String {
            let cells = terminal.backend().buffer().content();
            cells.iter().map(ratatui::buffer::Cell::symbol).collect()
        };
        crate::draw(&mut app, &mut terminal, &theme)?;
        assert!(screen(&terminal).contains("no kernel cells yet"));
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-notebook-late-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        let update = start("late", json!({"code": "late_cell = 1"}));
        let params = yi_types::acp::AcpUpdateParams {
            session_id: session.0,
            update,
        };
        app.reduce_update(&outbound, params);
        outbound.shutdown();
        app.dirty = true;
        crate::draw(&mut app, &mut terminal, &theme)?;
        assert!(
            screen(&terminal).contains("late_cell = 1"),
            "{}",
            screen(&terminal)
        );
        Ok(())
    }
    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("x{n}\n")).collect()
    }

    #[test]
    fn a_cell_with_a_tab_and_an_escape_reaches_no_span_raw() -> TestResult {
        let cell = NbCell {
            call_id: "c".to_owned(),
            number: 1,
            code: "if x:\n\ty = 1\x1b[31m\r".to_owned(),
            stdout: "a\tb\x1b[2J\rc".to_owned(),
            stderr: "e\tf\x1b]0;t\x07".to_owned(),
            result: Some("r\t1\x1b[0m".to_owned()),
            error: Some("Trace\tback\x1b[1m".to_owned()),
            ..NbCell::default()
        };
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = notebook_lines(&[cell], WIDTH, false, &theme, "⌘W");
        for span in lines.iter().flat_map(|line| line.spans.iter()) {
            let bad = span.content.chars().find(|c| c.is_control());
            assert_eq!(bad, None, "{:?}", span.content);
        }
        Ok(())
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
    fn media(sizes: &[usize]) -> Value {
        let items: Vec<Value> = sizes
            .iter()
            .map(|n| json!({"mime_type": "image/png", "data": "a".repeat(*n)}))
            .collect();
        json!({"attachmentMedia": items})
    }

    fn one_cell(details: Value) -> Vec<NbCell> {
        let mut cells = Vec::new();
        apply_notebook(&mut cells, &start("c1", json!({"code": "1"})));
        apply_notebook(&mut cells, &end("c1", details));
        cells
    }

    #[test]
    fn a_cells_text_is_capped_and_says_where() -> TestResult {
        let cells = one_cell(json!({
            "stdout": "x".repeat(8193),
            "stderr": "€".repeat(4000),
            "result": "y".repeat(2049),
        }));
        let cell = cells.first().ok_or("no cell")?;
        for (text, kept, total, cap) in [
            (cell.stdout.as_str(), 8192, 8193, 8),
            (cell.result.as_deref().unwrap_or_default(), 2048, 2049, 2),
            (cell.stderr.as_str(), 8190, 12000, 8),
        ] {
            let said = format!("[… {kept} of {total} bytes kept · notebook cap {cap} KiB");
            assert!(text.contains(&said), "{said:?} not in {text:?}");
        }
        assert_eq!(
            one_cell(json!({"stdout": "x".repeat(8192)}))[0].stdout,
            "x".repeat(8192),
            "a cap-sized stream is not cut"
        );
        Ok(())
    }

    #[test]
    fn dropped_images_are_recorded_by_the_cap_that_dropped_them() -> TestResult {
        let cells = one_cell(media(&[524289, 6, 6, 6, 6, 6, 6]));
        let cell = cells.first().ok_or("no cell")?;
        assert_eq!(cell.dropped_image_bytes, vec![524289], "over the size cap");
        assert_eq!(cell.images_past_limit, 2, "past the four-image limit");
        assert_eq!(cell.images.len(), 4);
        let at_limit = one_cell(media(&[524288, 6, 6, 6]));
        assert_eq!(at_limit[0].images.len(), 4, "the limit itself is kept");
        assert!(at_limit[0].dropped_image_bytes.is_empty());
        assert_eq!(at_limit[0].images_past_limit, 0);
        Ok(())
    }

    #[test]
    fn each_dropped_image_notice_names_its_own_cap() -> TestResult {
        let cells = one_cell(media(&[524289, 6, 6, 6, 6, 6, 6]));
        let said = rows(&cells, 80, false);
        assert!(
            said.iter().any(
                |row| row.contains("[…] image 524289 B base64 dropped · notebook cap 524288 B")
            ),
            "{said:#?}"
        );
        assert!(
            said.iter().any(|row| {
                row.contains("[…] 2 older images not shown · notebook keeps 4 per cell")
            }),
            "{said:#?}"
        );
        Ok(())
    }

    fn figures(count: usize) -> Result<Vec<NbCell>, Box<dyn std::error::Error>> {
        let recorded = session::session_entries(include_str!(
            "../../../tui/tests/fixtures/sessions/01a17148.jsonl"
        ))?;
        let mut cells = Vec::new();
        for n in 0..count {
            for mut update in recorded
                .iter()
                .flat_map(super::super::diffs::replayed_updates)
            {
                if let AcpSessionUpdate::ToolCallUpdate(call) = &mut update {
                    call.tool_call_id = format!("{}-{n}", call.tool_call_id);
                }
                apply_notebook(&mut cells, &update);
            }
        }
        Ok(cells)
    }

    /// Dies with every kept cell of a plotting loop holding its figure's base64, 13 MB for fifty
    /// 200 KB figures, or with a dropped figure losing its size row or a cap row.
    #[test]
    fn only_the_newest_figure_keeps_its_bytes_across_two_hundred_cells() -> TestResult {
        let cells = figures(200)?;
        assert_eq!(cells.len(), 50);
        let held: Vec<usize> = (cells.iter().enumerate())
            .filter(|(_, cell)| !cell.images.is_empty())
            .map(|(at, _)| at)
            .collect();
        assert_eq!(held, vec![49], "cells holding image data");
        let rows = rows(&cells, 80, false);
        let sized = rows
            .iter()
            .filter(|row| row.trim() == "▲ image · 198 KB png");
        assert_eq!(sized.count(), 50, "{rows:#?}");
        let cap = "[…] 150 older cells not shown · notebook keeps 50";
        assert!(rows.iter().any(|row| row.contains(cap)), "{rows:#?}");
        for (count, said) in [(50, false), (51, true)] {
            let rows = super::tests::rows(&figures(count)?, 80, false);
            assert_eq!(
                rows.iter().any(|row| row.contains("older cell")),
                said,
                "{count}"
            );
        }
        Ok(())
    }

    /// Dies with the oldest image of the newest cell placed, or with a cell's fifth image kept.
    #[test]
    fn the_newest_image_of_the_newest_cell_is_the_one_placed() -> TestResult {
        let mut cells = figures(1)?;
        let items = |data: &[&str]| {
            let items: Vec<Value> = (data.iter())
                .map(|data| json!({"mime_type": "image/png", "data": data}))
                .collect();
            json!({"attachmentMedia": items})
        };
        apply_notebook(&mut cells, &start("c-many", json!({"code": "plot()"})));
        let five = items(&["b2xkZXN0", "c2Vjb25k", "dGhpcmQ=", "Zm91cnRo", "bmV3ZXN0"]);
        apply_notebook(&mut cells, &end("c-many", five));
        let last = cells.last().ok_or("no cell")?;
        assert_eq!((last.images.len(), last.images_past_limit), (4, 1));
        let rows = rows(&cells, 80, false);
        let older = "[…] 1 older image not shown · notebook keeps 4 per cell";
        assert!(rows.iter().any(|row| row.contains(older)), "{rows:#?}");
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = crate::app::App::new("/r".to_owned(), theme);
        app.kitty = true;
        let session = crate::model::SessionId("s-plot".to_owned());
        let pane = app.state.focused_pane_mut().ok_or("no pane")?;
        pane.content = crate::model::PaneContent::Notebook {
            session: Some(session.clone()),
            full: false,
            drawn: None,
        };
        app.state.notebooks.insert(session, cells);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        app.dirty = true;
        crate::draw(&mut app, &mut terminal, &theme)?;
        let mut out = Vec::new();
        crate::place_notebook_image(&mut app, &mut out);
        let out = String::from_utf8_lossy(&out);
        assert!(
            out.contains(";bmV3ZXN0"),
            "the newest image was not placed: {out:?}"
        );
        Ok(())
    }

    fn said(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn only_marker(lines: &[Line]) -> Option<String> {
        let marked: Vec<String> = lines
            .iter()
            .map(said)
            .filter(|row| row.starts_with('✕'))
            .collect();
        assert_eq!(marked.len(), 1, "one marker per block: {marked:?}");
        marked
            .first()
            .map(|row| row.trim_start_matches('✕').trim().to_owned())
    }

    /// Dies with no marker at all: a bare `AssertionError` has no `Name: value` row to raise, so
    /// the marker fell on row 0, which the last-12 cap then cut away.
    #[test]
    fn a_capped_block_marks_the_bare_exception_it_ends_on() -> TestResult {
        let mut trace = "Traceback (most recent call last):\n".to_owned();
        for at in 0..20 {
            trace.push_str(&format!(
                "  File \"/work/p{at}.py\", line {at}, in f{at}\n    g{at}()\n"
            ));
        }
        trace.push_str("AssertionError\n");
        let cell = NbCell {
            number: 1,
            code: "assert ok".to_owned(),
            error: Some(trace),
            ..NbCell::default()
        };
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = notebook_lines(std::slice::from_ref(&cell), 100, false, &theme, "x");
        assert!(
            lines
                .iter()
                .map(said)
                .any(|row| row.contains("traceback last 12 of")),
            "the block is tail-capped: {:?}",
            lines.iter().map(said).collect::<Vec<_>>()
        );
        let marker = only_marker(&lines);
        assert!(
            marker.as_deref() == Some("AssertionError"),
            "the marker sits on the row naming the failure: {marker:?}"
        );
        Ok(())
    }

    /// Dies with a traceback drawn as one flat wall: its frame headers and the line each frame
    /// ran must not share a colour, and the failure marker must name the exception, once.
    #[test]
    fn a_recorded_traceback_dims_its_frames_and_lexes_what_each_ran() -> TestResult {
        let cells = replay(include_str!(
            "../../../tui/tests/fixtures/sessions/01a0648f.jsonl"
        ))?;
        let cell = cells
            .iter()
            .find(|cell| {
                cell.error
                    .as_deref()
                    .is_some_and(|e| e.contains("RuntimeError"))
            })
            .ok_or("no RuntimeError cell")?;
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = notebook_lines(std::slice::from_ref(cell), 100, true, &theme, "x");
        let frame = lines
            .iter()
            .find(|line| said(line).trim_start().starts_with("File "))
            .ok_or("no frame header row")?;
        let styled: Vec<_> = frame
            .spans
            .iter()
            .filter(|span| span.style.fg.is_some())
            .collect();
        assert!(!styled.is_empty(), "the header row is drawn");
        assert!(
            styled
                .iter()
                .all(|span| span.style.fg == theme.dim_style().fg),
            "a frame header is scaffolding: {:?}",
            frame.spans
        );
        let source = lines
            .iter()
            .find(|line| said(line).contains("return await result(self.rlm_child_id, schema)"))
            .ok_or("no source row")?;
        let keyword = theme.syntax_style(Token::Keyword).fg;
        assert!(
            source
                .spans
                .iter()
                .any(|span| span.style.fg == keyword && span.content.as_ref() == "return"),
            "the row a frame ran is lexed as code: {:?}",
            source.spans
        );
        let marker = only_marker(&lines);
        assert!(
            marker
                .as_deref()
                .is_some_and(|row| row.contains("RuntimeError: child")),
            "the marker sits on the row naming the failure: {marker:?}"
        );
        Ok(())
    }

    /// Dies with a `%%bash` cell's source lexed as Python, where `ls` and `head` are plain words.
    #[test]
    fn a_bash_cells_source_is_lexed_as_a_shell() -> TestResult {
        let cell = NbCell {
            number: 1,
            code: "%%bash\npwd && ls | head -5".to_owned(),
            ..NbCell::default()
        };
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = notebook_lines(std::slice::from_ref(&cell), 70, false, &theme, "x");
        let row = lines
            .iter()
            .find(|line| said(line).contains("pwd && ls"))
            .ok_or("no shell row")?;
        let function = theme.syntax_style(Token::Function).fg;
        assert!(
            row.spans
                .iter()
                .any(|span| span.style.fg == function && span.content.as_ref() == "ls"),
            "a shell command in a shell cell: {:?}",
            row.spans
        );
        Ok(())
    }

    /// Dies with the mime dropped between the tool's details and the cell: a declared markdown
    /// value drew as the text it arrived in.
    /// Dies with Yi's own byte clamp read as bad JSON: a 2 KiB JSON result would be drawn flat
    /// under a row claiming the value did not parse.
    #[test]
    fn a_json_result_past_the_bytes_names_the_cap_not_a_parse_failure() -> TestResult {
        let big = json!({"rows": vec!["tide"; 700]}).to_string();
        let drawn = rows(
            &one_cell(json!({"result": big, "resultMime": "application/json"})),
            84,
            false,
        )
        .join("\n");
        assert!(drawn.contains("past the cell's byte cap"), "{drawn}");
        assert!(!drawn.contains("did not parse as JSON"), "{drawn}");
        let broken = rows(
            &one_cell(json!({"result": "{not json", "resultMime": "application/json"})),
            84,
            false,
        )
        .join("\n");
        assert!(broken.contains("did not parse as JSON"), "{broken}");
        Ok(())
    }

    #[test]
    fn a_cells_details_hand_its_mime_to_the_pane() -> TestResult {
        let cells = one_cell(json!({"result": "# Tide", "resultMime": "text/markdown"}));
        assert_eq!(
            cells.first().and_then(|cell| cell.result_mime.as_deref()),
            Some("text/markdown")
        );
        let drawn = rows(&cells, 72, false);
        assert!(drawn.iter().any(|row| row.contains("Tide")), "{drawn:#?}");
        assert!(
            !drawn.iter().any(|row| row.contains("# Tide")),
            "{drawn:#?}"
        );
        Ok(())
    }

    /// The recorded reply that carries every markdown block a notebook must draw: a heading, a
    /// numbered list, bold, a table and a fence.
    const MARKDOWN_SESSION: &str =
        include_str!("../../../tui/tests/fixtures/sessions/01a0d6e4.jsonl");

    fn recorded_markdown() -> Result<String, Box<dyn std::error::Error>> {
        let mut best = String::new();
        for message in session::session_entries(MARKDOWN_SESSION)? {
            if let AgentMessage::Assistant { content, .. } = &message {
                for block in content {
                    if let Content::Text { text, .. } = block
                        && text.len() > best.len()
                    {
                        best.clone_from(text);
                    }
                }
            }
        }
        Ok(best)
    }

    /// A recorded kernel result's details, which is the JSON object a notebook renders as a
    /// tree: a real payload, in the shape the tool wrote it.
    fn recorded_json() -> Result<String, serde_json::Error> {
        let details = session::session_entries(RECORDED)?
            .into_iter()
            .find_map(|message| match message {
                AgentMessage::ToolResult {
                    tool_name, details, ..
                } if tool_name == "ipython" => details,
                _ => None,
            })
            .unwrap_or_default();
        serde_json::to_string(&details)
    }

    fn kind_cell(mime: Option<&str>, result: &str) -> NbCell {
        NbCell {
            number: 1,
            result: Some(result.to_owned()),
            result_mime: mime.map(str::to_owned),
            ..NbCell::default()
        }
    }

    /// Dies with a markdown result drawn raw: the heading keeps its `#`, the table its pipes,
    /// and the chat's own renderer never ran.
    #[test]
    fn a_markdown_result_draws_its_blocks_as_the_chat_does() -> TestResult {
        let source = recorded_markdown()?;
        assert!(source.len() > 4_000, "the recorded reply is life-sized");
        let drawn = rows(&[kind_cell(Some("text/markdown"), &source)], 72, true);
        for (raw, drawn_as) in [
            ("# Yi codebase assessment", "Yi codebase assessment"),
            ("## What it is", "What it is"),
        ] {
            assert!(source.contains(raw), "{raw:?} left the recording");
            assert!(
                drawn.iter().any(|row| row.contains(drawn_as)),
                "{drawn_as:?}: {drawn:#?}"
            );
            let raw_row = drawn.iter().find(|row| row.contains(raw));
            assert_eq!(raw_row, None, "{raw:?} drawn raw");
        }
        assert!(
            drawn.iter().any(|row| row.contains("│ dimension")),
            "no table row: {drawn:#?}"
        );
        assert!(
            !drawn.iter().any(|row| row.contains("|---|")),
            "the table's rule drawn raw"
        );
        assert!(
            drawn
                .iter()
                .any(|row| row.contains("1. Crate line budgets")),
            "no list row"
        );
        Ok(())
    }

    /// Dies with a JSON result drawn as its one compact line, or with the cap row silent.
    #[test]
    fn a_json_result_draws_pretty_and_names_the_cap_it_collapsed_past() -> TestResult {
        let source = recorded_json()?;
        let drawn = rows(&[kind_cell(Some("application/json"), &source)], 72, true);
        assert!(
            drawn.iter().any(|row| row.contains("  \"stdout\":")),
            "not pretty-printed: {drawn:#?}"
        );
        assert!(
            !drawn.iter().any(|row| row.contains("{\"status\":")),
            "the compact one-line source was drawn"
        );
        let long = json!({"crates": (1..=40).map(|n| format!("crate-{n}")).collect::<Vec<_>>()});
        let cut = rows(
            &[kind_cell(Some("application/json"), &long.to_string())],
            72,
            false,
        );
        let notice = cut
            .iter()
            .find(|row| row.contains("[…] result first 20 of"))
            .ok_or("cap row silent")?;
        assert!(notice.contains("notebook cap 20"), "{notice:?}");
        assert!(notice.contains("v shows every row"), "{notice:?}");
        let whole = rows(
            &[kind_cell(Some("application/json"), &long.to_string())],
            72,
            true,
        );
        assert!(
            whole.iter().any(|row| row.contains("crate-40")),
            "{whole:#?}"
        );
        Ok(())
    }

    /// Dies with an unknown mime re-routed: a type no renderer names draws the value as the
    /// text it already was, byte for byte beside a cell that named no mime at all.
    #[test]
    fn a_mime_no_renderer_names_draws_the_value_as_text() -> TestResult {
        let source = "# not a heading\n| a | b |\n{\"one\": 1}";
        let plain = rows(&[kind_cell(None, source)], 72, false);
        for mime in ["text/html", "text/plain", "image/svg+xml"] {
            assert_eq!(
                rows(&[kind_cell(Some(mime), source)], 72, false),
                plain,
                "{mime} was re-routed"
            );
        }
        Ok(())
    }

    /// Dies with a python result drawn flat: its keyword keeps the theme's text colour.
    #[test]
    fn a_code_result_is_highlighted_by_the_language_its_mime_names() -> TestResult {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let keyword = |mime: Option<&str>| -> Option<Option<ratatui::style::Color>> {
            notebook_lines(
                &[kind_cell(mime, "def tide(sea):\n    return sea")],
                72,
                false,
                &theme,
                "⌘W",
            )
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == "def")
            .map(|span| span.style.fg)
        };
        let coloured = keyword(Some("text/x-python")).ok_or("no def span")?;
        assert_ne!(coloured, Some(theme.text), "the keyword was drawn flat");
        assert_eq!(keyword(None), None, "an unnamed mime was split into tokens");
        Ok(())
    }

    /// Dies with a declared JSON value that is not JSON drawn pretty anyway, or with no row
    /// saying the pane fell back to text.
    #[test]
    fn a_json_result_that_does_not_parse_says_it_drew_as_text() -> TestResult {
        let drawn = rows(
            &[kind_cell(Some("application/json"), "{\"cut\": ")],
            72,
            false,
        );
        assert!(
            drawn
                .iter()
                .any(|row| row
                    .contains("application/json result did not parse as JSON · shown as text")),
            "{drawn:#?}"
        );
        assert!(
            drawn.iter().any(|row| row.contains("{\"cut\":")),
            "the value itself is gone"
        );
        Ok(())
    }

    /// Dies with a markdown value keeping its `##`, or a tab or an escape drawn raw.
    #[test]
    fn every_result_kind_survives_an_empty_and_a_control_ridden_value() -> TestResult {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mimes = [
            None,
            Some("text/markdown"),
            Some("application/json"),
            Some("text/x-python"),
        ];
        for mime in mimes {
            let empty = notebook_lines(&[kind_cell(mime, "")], 72, false, &theme, "⌘W");
            assert!(
                !empty
                    .iter()
                    .flat_map(|line| line.spans.iter())
                    .any(|span| span.content.contains('…')),
                "{mime:?} drew a cut row for an empty value"
            );
            let cell = kind_cell(mime, "## a\tb\x1b[2J\rc € {\"k\": ");
            let lines = notebook_lines(&[cell], 72, false, &theme, "⌘W");
            for span in lines.iter().flat_map(|line| line.spans.iter()) {
                assert_eq!(
                    span.content.chars().find(|c: &char| c.is_control()),
                    None,
                    "{mime:?}: {:?}",
                    span.content
                );
            }
            let drawn: String = lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(
                drawn.contains("##"),
                mime != Some("text/markdown"),
                "{mime:?} was not drawn by its mime: {drawn}"
            );
        }
        Ok(())
    }
}
