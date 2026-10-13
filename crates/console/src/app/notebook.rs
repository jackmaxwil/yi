use serde_json::Value;
use yi_types::acp::{AcpContentBlock, AcpSessionUpdate, AcpToolCallUpdate, AcpToolContent};

fn error_text(error: &Value) -> Option<String> {
    let traceback: Vec<&str> = error
        .get("traceback")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !traceback.is_empty() {
        return Some(traceback.join("\n"));
    }
    let name = error.get("ename").and_then(Value::as_str)?;
    let value = error
        .get("evalue")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(format!("{name}: {value}"))
}

/// Kernel cells out of the tool-call stream: the start carries the code,
/// the end carries streams and attachments in the details record.
pub fn apply_notebook(cells: &mut Vec<crate::model::NbCell>, update: &AcpSessionUpdate) -> bool {
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
                    .unwrap_or_default()
                    .to_owned();
                cell.result = details
                    .get("result")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned);
                cell.stderr = details
                    .get("stderr")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                cell.duration_ms = details.get("durationMs").and_then(Value::as_u64);
                cell.error = details.get("error").and_then(error_text);
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
                cell.images = legacy
                    .chain(current)
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
            if status.is_some() || raw_output.is_some() {
                cell.running = false;
            }
            true
        }
        (None, false) => false,
    }
}
