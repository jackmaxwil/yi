use serde_json::Value;
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};

pub fn tool_cell(
    name: &str,
    argument: &str,
    result: &str,
    details: &Value,
    failed: bool,
) -> ToolCell {
    ToolCell {
        name: name.to_owned(),
        call_id: String::new(),
        status: if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        },
        summary: ToolCell::summary_of(name, argument),
        digest: ToolCell::digest_of(name, result, details, failed),
        preview: result.lines().map(str::to_owned).collect(),
        elapsed_ms: 0,
        calls: 1,
        details: details.clone(),
    }
}

pub fn rendered(cell: &ToolCell, width: usize, mode: TranscriptMode) -> Vec<String> {
    Cell::Tool(cell.clone())
        .lines(width, &Theme::new(ColorTier::Ansi16, true), mode, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}
