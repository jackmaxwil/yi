//! A message in the plain terms a pane draws, read from its typed form.

use yi_types::message::{AgentMessage, Content, StopReason, UserContent};

use crate::reduce::turns::{Body, Message};

/// One block of what was said, as a pane draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    /// A tool call, by its id.
    Call(String),
}

/// A message in the plain terms a pane reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Said {
    pub user: bool,
    pub blocks: Vec<Block>,
    /// `provider/model`, for an answer.
    pub model: String,
    /// Output tokens, for an answer.
    pub output: u64,
    /// An answer the user's Stop cut short.
    pub stopped: bool,
}

impl Message {
    /// What the message says; none for a kind no pane shows, a tool's result among them.
    pub fn said(&self) -> Option<Said> {
        let Body::Message(body) = &self.body else {
            return None;
        };
        match body.as_ref() {
            AgentMessage::User { content, .. } => Some(Said {
                user: true,
                blocks: match content {
                    UserContent::Text(words) => vec![Block::Text(words.clone())],
                    UserContent::Blocks(blocks) => plain(blocks),
                },
                ..Said::default()
            }),
            AgentMessage::Assistant {
                content,
                provider,
                model,
                stop_reason,
                ..
            } => Some(Said {
                user: false,
                blocks: plain(content),
                model: format!("{provider}/{model}"),
                output: self.spent().map_or(0, |spent| spent.output),
                stopped: matches!(stop_reason, StopReason::Aborted),
            }),
            _ => None,
        }
    }
}

fn plain(content: &[Content]) -> Vec<Block> {
    let blocks = content.iter().filter_map(|block| match block {
        Content::Text { text, .. } => Some(Block::Text(text.clone())),
        Content::Thinking { thinking, .. } => Some(Block::Thinking(thinking.clone())),
        Content::ToolCall { id, .. } => Some(Block::Call(id.clone())),
        Content::Image { .. } | Content::Other(_) => None,
    });
    blocks.collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::Block;
    use crate::model::Model;
    use crate::wire::replay::play;

    /// A real turn off the wire: a prompt, eight tool calls and the word that ends it.
    const TOOLS: &str = include_str!("../../../../fixtures/wire/tools-turn.jsonl");

    #[test]
    fn a_recorded_turn_reads_as_a_prompt_its_calls_and_its_answer() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        play(&mut model, TOOLS).map_err(|e| e.to_string())?;
        let thread = model.wire.threads.values().next().ok_or("no thread")?;
        let read: Vec<_> = thread
            .transcript
            .messages
            .iter()
            .map(|m| m.said())
            .collect();
        let said: Vec<_> = read.iter().flatten().collect();
        let prompt = said.first().ok_or("nothing was said")?;
        assert!(prompt.user && prompt.model.is_empty());
        assert!(
            matches!(prompt.blocks.as_slice(), [Block::Text(words)] if words.starts_with("Make exactly these tool calls")),
            "the prompt is the words typed: {:?}",
            prompt.blocks
        );
        let calls: Vec<&str> = said
            .iter()
            .flat_map(|said| &said.blocks)
            .filter_map(|block| match block {
                Block::Call(id) => Some(id.as_str()),
                Block::Text(_) | Block::Thinking(_) => None,
            })
            .collect();
        assert_eq!(calls, ["t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8"]);
        let last = said.last().ok_or("nothing was said")?;
        assert_eq!(last.model, "faux/faux-1");
        assert!(!last.user && !last.stopped);
        assert!(matches!(last.blocks.as_slice(), [Block::Text(words)] if words.contains("done")));
        Ok(())
    }
}
