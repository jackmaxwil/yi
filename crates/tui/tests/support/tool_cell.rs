use serde_json::Value;
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
pub fn flat<L: ToString>(lines: &[L]) -> Vec<String> {
    lines.iter().map(ToString::to_string).collect()
}

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
    flat(&Cell::Tool(cell.clone()).lines(width, &Theme::new(ColorTier::Ansi16, true), mode, 0))
}
