use ratatui::text::Line;
use yi_tui::cell::{ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::Theme;
use yi_types::acp::{AcpContentBlock, AcpSessionUpdate, AcpToolCallStatus, AcpToolContent};

/// Retained lines per pane; the full history lives in the worker's JsonlRepo, so dropping the
/// oldest blocks loses only local scrollback. ponytail: fixed cap, configurable if asked.
const MAX_LINES: usize = 10_000;

/// Invariant: committed blocks render once into the cache; only the open
/// streaming block re-renders per delta, never the whole transcript.
enum Block {
    User {
        text: String,
    },
    Assistant {
        message_id: String,
        markdown: String,
        open: bool,
    },
    Thought {
        message_id: String,
        markdown: String,
        open: bool,
    },
    Tool(ToolCell),
    Note {
        text: String,
    },
}

struct Slot {
    block: Block,
    cache: Option<Vec<Line<'static>>>,
}

pub struct Transcript {
    slots: Vec<Slot>,
    width: usize,
    line_total: usize,
}

fn text_of(blocks: &[AcpContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            AcpContentBlock::Text { text } => Some(text.as_str()),
            AcpContentBlock::Other(_) => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn chunk_text(content: &AcpContentBlock) -> &str {
    match content {
        AcpContentBlock::Text { text } => text,
        AcpContentBlock::Other(_) => "",
    }
}

fn tool_status(status: &AcpToolCallStatus) -> ToolStatus {
    match status {
        AcpToolCallStatus::Pending | AcpToolCallStatus::InProgress => ToolStatus::Running,
        AcpToolCallStatus::Completed => ToolStatus::Done,
        AcpToolCallStatus::Failed => ToolStatus::Failed,
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            width: 80,
            line_total: 0,
        }
    }

    /// Invariant: wiped when `session/resume` is SENT — replay updates
    /// stream ahead of the resume response.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.line_total = 0;
    }

    pub fn set_width(&mut self, width: usize) {
        let width = width.max(10);
        if width != self.width {
            self.width = width;
            self.line_total = 0;
            for slot in &mut self.slots {
                slot.cache = None;
            }
        }
    }

    pub fn note(&mut self, text: String) {
        self.close_streams();
        self.push(Block::Note { text });
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Apply one focused-session update; true when the frame is dirty.
    pub fn apply(&mut self, update: &AcpSessionUpdate) -> bool {
        match update {
            AcpSessionUpdate::AgentMessageChunk {
                message_id,
                content,
            } => {
                self.append_stream(message_id, chunk_text(content), false);
                true
            }
            AcpSessionUpdate::AgentThoughtChunk {
                message_id,
                content,
            } => {
                self.append_stream(message_id, chunk_text(content), true);
                true
            }
            AcpSessionUpdate::AgentMessage {
                message_id,
                content,
            } => {
                self.close_streams();
                self.push(Block::Assistant {
                    message_id: message_id.clone(),
                    markdown: text_of(content),
                    open: false,
                });
                true
            }
            AcpSessionUpdate::UserMessage { content, .. } => {
                self.close_streams();
                self.push(Block::User {
                    text: text_of(content),
                });
                true
            }
            AcpSessionUpdate::StateUpdate(_) => {
                self.close_streams();
                false
            }
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title,
                status,
                content,
                raw_output,
                ..
            } => {
                self.upsert_tool(
                    tool_call_id,
                    title.as_deref(),
                    status.as_ref(),
                    content,
                    raw_output,
                );
                true
            }
            AcpSessionUpdate::ToolCallContentChunk {
                tool_call_id,
                content,
            } => {
                self.tool_chunk(tool_call_id, content);
                true
            }
            AcpSessionUpdate::TerminalUpdate { .. }
            | AcpSessionUpdate::TerminalOutputChunk { .. } => false,
            AcpSessionUpdate::UsageUpdate { .. } => false,
            AcpSessionUpdate::Extension(extension) => {
                if extension.session_update == "_yi/compaction" {
                    self.push(Block::Note {
                        text: "· context compacted ·".to_owned(),
                    });
                    return true;
                }
                false
            }
        }
    }

    fn push(&mut self, block: Block) {
        self.slots.push(Slot { block, cache: None });
        self.trim();
    }

    fn trim(&mut self) {
        while self.line_total > MAX_LINES && self.slots.len() > 1 {
            let removed = self.slots.remove(0);
            if let Some(cache) = removed.cache {
                self.line_total = self.line_total.saturating_sub(cache.len());
            }
        }
    }

    fn close_streams(&mut self) {
        for slot in &mut self.slots {
            match &mut slot.block {
                Block::Assistant { open, .. } | Block::Thought { open, .. } => *open = false,
                Block::User { .. } | Block::Tool(_) | Block::Note { .. } => {}
            }
        }
    }

    fn append_stream(&mut self, message_id: &str, delta: &str, thought: bool) {
        if delta.is_empty() {
            return;
        }
        if let Some(slot) = self.slots.last_mut() {
            let matched = match (&mut slot.block, thought) {
                (
                    Block::Assistant {
                        message_id: id,
                        markdown,
                        open,
                    },
                    false,
                )
                | (
                    Block::Thought {
                        message_id: id,
                        markdown,
                        open,
                    },
                    true,
                ) if id == message_id && *open => {
                    markdown.push_str(delta);
                    true
                }
                _ => false,
            };
            if matched {
                slot.cache = None;
                return;
            }
        }
        let block = if thought {
            Block::Thought {
                message_id: message_id.to_owned(),
                markdown: delta.to_owned(),
                open: true,
            }
        } else {
            Block::Assistant {
                message_id: message_id.to_owned(),
                markdown: delta.to_owned(),
                open: true,
            }
        };
        self.push(block);
    }

    fn upsert_tool(
        &mut self,
        call_id: &str,
        title: Option<&str>,
        status: Option<&AcpToolCallStatus>,
        content: &Option<Vec<AcpToolContent>>,
        raw_output: &Option<serde_json::Value>,
    ) {
        let digest = content.as_ref().and_then(|items| {
            items.iter().find_map(|item| match item {
                AcpToolContent::Content {
                    content: AcpContentBlock::Text { text },
                } => text.lines().next().map(str::to_owned),
                AcpToolContent::Terminal { .. } | AcpToolContent::Diff { .. } => None,
                AcpToolContent::Content { .. } => None,
            })
        });
        for slot in self.slots.iter_mut().rev() {
            if let Block::Tool(cell) = &mut slot.block
                && cell.call_id == call_id
            {
                if let Some(status) = status {
                    cell.status = tool_status(status);
                }
                if let Some(digest) = digest {
                    cell.digest = Some(digest);
                }
                if let Some(details) = raw_output {
                    cell.details = details.clone();
                }
                slot.cache = None;
                return;
            }
        }
        let name = title.unwrap_or("tool").to_owned();
        self.close_streams();
        self.push(Block::Tool(ToolCell {
            summary: name.clone(),
            name,
            call_id: call_id.to_owned(),
            intent: None,
            status: status.map_or(ToolStatus::Running, tool_status),
            digest,
            preview: Vec::new(),
            elapsed_ms: 0,
            calls: 1,
            details: raw_output.clone().unwrap_or(serde_json::Value::Null),
        }));
    }

    fn tool_chunk(&mut self, call_id: &str, content: &AcpToolContent) {
        let text = match content {
            AcpToolContent::Content {
                content: AcpContentBlock::Text { text },
            } => text.as_str(),
            _ => return,
        };
        for slot in self.slots.iter_mut().rev() {
            if let Block::Tool(cell) = &mut slot.block
                && cell.call_id == call_id
            {
                for line in text
                    .lines()
                    .take(5_usize.saturating_sub(cell.preview.len()))
                {
                    cell.preview.push(line.to_owned());
                }
                slot.cache = None;
                return;
            }
        }
    }

    /// All lines at the current width, rendering only uncached slots. Invariant: the retained
    /// count is re-totalled from the caches, since an open block re-renders many times.
    pub fn lines(&mut self, theme: &Theme) -> Vec<Line<'static>> {
        let width = self.width;
        let mut out = Vec::new();
        let mut total = 0_usize;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.cache.is_none() {
                slot.cache = Some(render_block(&slot.block, width, theme));
            }
            if let Some(cache) = &slot.cache {
                total = total.saturating_add(cache.len());
                if index > 0 {
                    out.push(Line::default());
                }
                out.extend(cache.iter().cloned());
            }
        }
        self.line_total = total;
        out
    }
}

fn render_block(block: &Block, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    match block {
        Block::User { text } => {
            let mut lines = Vec::new();
            for (index, raw) in text.lines().enumerate() {
                let prefix = if index == 0 { "❯ " } else { "  " };
                let content = format!("{prefix}{raw}");
                for wrapped in yi_tui::wrap::wrap_line(
                    &Line::styled(content, theme.user_style().patch(theme.accent_style())),
                    width,
                    "  ",
                ) {
                    lines.push(wrapped);
                }
            }
            if lines.is_empty() {
                lines.push(Line::styled("❯", theme.accent_style()));
            }
            lines
        }
        Block::Assistant { markdown, .. } => yi_tui::markdown::render(markdown, width, theme),
        Block::Thought { markdown, .. } => {
            yi_tui::cell::thought_lines(markdown, width, theme, TranscriptMode::Thinking, true)
        }
        Block::Tool(cell) => cell.lines(width, theme, TranscriptMode::Thinking, 0),
        Block::Note { text } => vec![Line::styled(text.clone(), theme.dim_style())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_tui::colors::ColorTier;

    #[test]
    fn streaming_rerenders_keep_the_retained_count_honest() {
        let theme = Theme::new(ColorTier::Ansi16, true);
        let mut transcript = Transcript::new();
        transcript.apply(&AcpSessionUpdate::UserMessage {
            message_id: "u1".to_owned(),
            content: vec![AcpContentBlock::Text {
                text: "first question".to_owned(),
            }],
        });
        for _ in 0..500 {
            transcript.apply(&AcpSessionUpdate::AgentMessageChunk {
                message_id: "a1".to_owned(),
                content: AcpContentBlock::Text {
                    text: "answer line\n".to_owned(),
                },
            });
            let _ = transcript.lines(&theme);
        }
        // The next push is where an inflated count would trim live blocks.
        transcript.apply(&AcpSessionUpdate::UserMessage {
            message_id: "u2".to_owned(),
            content: vec![AcpContentBlock::Text {
                text: "second question".to_owned(),
            }],
        });
        let lines = transcript.lines(&theme);
        assert_eq!(transcript.slots.len(), 3, "no block may be trimmed here");
        // Three blocks, two blank separators: the count must be exact.
        assert_eq!(transcript.line_total + 2, lines.len());
    }
}
