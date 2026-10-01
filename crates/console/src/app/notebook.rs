use serde_json::Value;
use yi_types::acp::{AcpContentBlock, AcpSessionUpdate, AcpToolCallUpdate, AcpToolContent};

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
            cells.push(crate::model::NbCell {
                call_id: tool_call_id.clone(),
                code,
                running: true,
                ..crate::model::NbCell::default()
            });
            if cells.len() > 50 {
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
                cell.error = details
                    .pointer("/error/evalue")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
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
