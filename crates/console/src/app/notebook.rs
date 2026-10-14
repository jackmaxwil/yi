use serde_json::Value;
use yi_types::acp::{AcpContentBlock, AcpSessionUpdate, AcpToolCallUpdate, AcpToolContent};

const STDOUT_CAP: usize = 8 * 1024;
const RESULT_CAP: usize = 2 * 1024;
const IMAGE_CAP: usize = 64 * 1024;
const IMAGE_MAX: usize = 4;

fn clamp_bytes(text: &str, cap: usize) -> String {
    yi_types::text::truncate_utf8(text, cap).to_owned()
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
                    .map(|text| clamp_bytes(text, STDOUT_CAP))
                    .unwrap_or_default();
                cell.result = details
                    .get("result")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| clamp_bytes(text, RESULT_CAP));
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
                let all: Vec<&str> = legacy.chain(current).filter_map(Value::as_str).collect();
                cell.images_dropped = all
                    .iter()
                    .filter(|data| data.len() > IMAGE_CAP)
                    .map(|data| data.len())
                    .collect();
                let images: Vec<String> = all
                    .into_iter()
                    .filter(|data| data.len() <= IMAGE_CAP)
                    .map(str::to_owned)
                    .collect();
                let skip = images.len().saturating_sub(IMAGE_MAX);
                cell.images_dropped
                    .extend(images[..skip].iter().map(|data| data.len()));
                cell.images = images.into_iter().skip(skip).collect();
            }
            if status.is_some() || raw_output.is_some() {
                cell.running = false;
            }
            true
        }
        (None, false) => false,
    }
}
#[cfg(test)]
mod tests {
    use yi_types::acp::AcpToolCallStatus;

    use super::*;

    fn start(id: &str) -> AcpSessionUpdate {
        AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
            tool_call_id: id.to_owned(),
            title: Some("ipython".to_owned()),
            raw_input: Some(serde_json::json!({"code": "1 + 1"})),
            ..AcpToolCallUpdate::default()
        })
    }

    fn end(id: &str, details: serde_json::Value) -> AcpSessionUpdate {
        AcpSessionUpdate::ToolCallUpdate(AcpToolCallUpdate {
            tool_call_id: id.to_owned(),
            status: Some(AcpToolCallStatus::Completed),
            raw_output: Some(details),
            ..AcpToolCallUpdate::default()
        })
    }

    #[test]
    fn a_cells_stdout_is_capped() -> Result<(), Box<dyn std::error::Error>> {
        let mut cells = Vec::new();
        assert!(apply_notebook(&mut cells, &start("c1")));
        let big = "x".repeat(8193);
        assert!(apply_notebook(
            &mut cells,
            &end("c1", serde_json::json!({"stdout": big}))
        ));
        assert_eq!(cells[0].stdout.chars().count(), 8192, "stdout capped");
        Ok(())
    }

    #[test]
    fn a_cells_result_is_capped() -> Result<(), Box<dyn std::error::Error>> {
        let mut cells = Vec::new();
        assert!(apply_notebook(&mut cells, &start("c1")));
        let big = "y".repeat(2049);
        assert!(apply_notebook(
            &mut cells,
            &end("c1", serde_json::json!({"stdout": "ok", "result": big}))
        ));
        assert_eq!(cells[0].result.as_deref().map(str::len), Some(2048));
        Ok(())
    }

    #[test]
    fn a_cells_images_are_capped() -> Result<(), Box<dyn std::error::Error>> {
        let mut cells = Vec::new();
        assert!(apply_notebook(&mut cells, &start("c1")));
        let big = "a".repeat(65537);
        let media: Vec<_> = std::iter::once(big)
            .chain((0..5).map(|i| format!("small{i}")))
            .map(|data| serde_json::json!({"mime_type": "image/png", "data": data}))
            .collect();
        assert!(apply_notebook(
            &mut cells,
            &end("c1", serde_json::json!({"attachmentMedia": media}))
        ));
        assert_eq!(cells[0].images.len(), 4, "image count capped");
        assert!(cells[0].images.iter().all(|image| image.len() <= 65536));
        assert_eq!(cells[0].images.last().map(String::as_str), Some("small4"));
        Ok(())
    }

    #[test]
    fn a_cells_dropped_images_are_recorded() -> Result<(), Box<dyn std::error::Error>> {
        let mut cells = Vec::new();
        assert!(apply_notebook(&mut cells, &start("c1")));
        let big = "a".repeat(65537);
        let media: Vec<_> = std::iter::once(big)
            .chain((0..6).map(|i| format!("small{i}")))
            .map(|data| serde_json::json!({"mime_type": "image/png", "data": data}))
            .collect();
        assert!(apply_notebook(
            &mut cells,
            &end("c1", serde_json::json!({"attachmentMedia": media}))
        ));
        assert_eq!(cells[0].images_dropped, vec![65537, 6, 6]);
        Ok(())
    }
}
