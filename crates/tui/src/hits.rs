use ratatui::layout::Rect;
use yi_types::config::RewriteLength;

/// What a pane's last paint drew that a press or a hover can land on.
#[derive(Default)]
pub struct ChatHits {
    /// Each run chip: where it stands, its cell and its ordinal among that cell's shell blocks.
    pub runs: Vec<(Rect, usize, usize)>,
    pub improve: Vec<(Rect, Option<RewriteLength>)>,
}
