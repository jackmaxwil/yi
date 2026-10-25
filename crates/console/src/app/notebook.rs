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

    fn recorded() -> Result<Vec<NbCell>, serde_json::Error> {
        replay(include_str!(
            "../../../tui/tests/fixtures/sessions/01a13357.jsonl"
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
}
