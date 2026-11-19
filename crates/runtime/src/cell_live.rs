use std::sync::{Mutex, PoisonError};

use std::sync::Arc;

use serde_json::json;
use tokio::sync::broadcast::Sender;
use yi_kernel::client::StreamFn;
use yi_tools::cut_row;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::Content;

pub(crate) const LIVE_CHUNK_CHARS: usize = 8192;

struct Piece {
    stream: &'static str,
    text: String,
    kept: usize,
    seen: usize,
}

/// A running cell's output between two ticks, sent as one event per stream so a chatty cell
/// costs the socket ten events a second, not one per `print`. The result stays authoritative.
pub(crate) struct LiveOutput {
    call_id: String,
    events: Sender<AgentEvent>,
    pending: Mutex<Pending>,
}

struct Pending {
    pieces: Vec<Piece>,
    closed: bool,
}

impl LiveOutput {
    pub(crate) fn new(call_id: String, events: Sender<AgentEvent>) -> Self {
        Self {
            call_id,
            events,
            pending: Mutex::new(Pending {
                pieces: Vec::new(),
                closed: false,
            }),
        }
    }

    pub(crate) fn push(&self, name: &str, text: &str) {
        let stream = match name {
            "stdout" => "stdout",
            "stderr" => "stderr",
            _ => return,
        };
        let mut state = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return;
        }
        let pending = &mut state.pieces;
        let at = match pending.iter().position(|piece| piece.stream == stream) {
            Some(at) => at,
            None => {
                pending.push(Piece {
                    stream,
                    text: String::new(),
                    kept: 0,
                    seen: 0,
                });
                pending.len().saturating_sub(1)
            }
        };
        let Some(piece) = pending.get_mut(at) else {
            return;
        };
        piece.seen = piece.seen.saturating_add(text.chars().count());
        let room = LIVE_CHUNK_CHARS.saturating_sub(piece.kept);
        let head: String = text.chars().take(room).collect();
        piece.kept = piece.kept.saturating_add(head.chars().count());
        piece.text.push_str(&head);
    }

    pub(crate) fn flush(&self) {
        self.drain(false);
    }

    pub(crate) fn close(&self) {
        self.drain(true);
    }

    fn drain(&self, close: bool) {
        let mut state = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.closed |= close;
        for piece in std::mem::take(&mut state.pieces) {
            let mut text = piece.text;
            if piece.seen > piece.kept {
                text.push('\n');
                text.push_str(&cut_row(
                    piece.kept,
                    piece.seen,
                    "chars shown live per tick",
                    "LIVE_CHUNK_CHARS",
                    LIVE_CHUNK_CHARS,
                    "the full output arrives with the cell's result",
                ));
            }
            let _ = self.events.send(AgentEvent::ToolExecutionUpdate {
                tool_call_id: self.call_id.clone(),
                tool_name: "ipython".to_owned(),
                args: json!({}),
                partial_result: ToolResult {
                    content: vec![Content::Text {
                        text,
                        text_signature: None,
                    }],
                    details: json!({ "stream": piece.stream }),
                    usage: None,
                    added_tool_names: None,
                    terminate: None,
                },
            });
        }
    }
}

impl crate::kernel::KernelService {
    /// Where a running cell's output goes while it runs: the session's event stream.
    pub fn attach_events(&self, events: Sender<AgentEvent>) {
        self.events.get_or_init(|| events);
    }

    /// Streams the cell `running_cell` names, and nothing for a cell no tool call asked for.
    pub(crate) fn live_cell(&self) -> LiveCell {
        let call = (self.running_cell.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let out = self
            .events
            .get()
            .zip(call)
            .map(|(events, call)| Arc::new(LiveOutput::new(call, events.clone())));
        let tick = out.clone().map(|out| {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    out.flush();
                }
            })
        });
        LiveCell { out, tick }
    }
}

/// Invariant: dropped when the cell ends, which stops the tick and sends the last piece
/// before the tool's end event can follow.
pub(crate) struct LiveCell {
    out: Option<Arc<LiveOutput>>,
    tick: Option<tokio::task::JoinHandle<()>>,
}

impl LiveCell {
    pub(crate) fn tee(&self, mut inner: Box<StreamFn>) -> Box<StreamFn> {
        let out = self.out.clone();
        Box::new(move |name, text| {
            if let Some(out) = &out {
                out.push(name, text);
            }
            inner(name, text);
        })
    }
}

impl Drop for LiveCell {
    fn drop(&mut self) {
        if let Some(tick) = &self.tick {
            tick.abort();
        }
        if let Some(out) = &self.out {
            out.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> Vec<(String, String)> {
        let mut seen = Vec::new();
        while let Ok(AgentEvent::ToolExecutionUpdate { partial_result, .. }) = rx.try_recv() {
            let text = match partial_result.content.first() {
                Some(Content::Text { text, .. }) => text.clone(),
                _ => String::new(),
            };
            seen.push((partial_result.details["stream"].to_string(), text));
        }
        seen
    }

    #[test]
    fn a_thousand_prints_in_one_tick_are_one_event_per_stream() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let live = LiveOutput::new("c1".to_owned(), tx);
        for n in 0..1000 {
            live.push("stdout", &format!("line {n}\n"));
        }
        live.push("stderr", "warn\n");
        live.push("execute_result", "ignored");
        live.flush();
        let seen = drain(&mut rx);
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].1.starts_with("line 0\n"), "{seen:?}");
        assert_eq!(seen[1].1, "warn\n");
        live.flush();
        assert!(drain(&mut rx).is_empty(), "an idle tick sends nothing");
    }

    #[test]
    fn a_flood_past_the_cap_says_what_was_kept_and_where_the_rest_is() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let live = LiveOutput::new("c1".to_owned(), tx);
        let seen_chars = LIVE_CHUNK_CHARS + 5;
        live.push("stdout", &"é".repeat(seen_chars));
        live.flush();
        let seen = drain(&mut rx);
        let text = &seen[0].1;
        assert!(text.starts_with(&"é".repeat(LIVE_CHUNK_CHARS)), "{text}");
        assert!(
            text.contains(&format!("[… {LIVE_CHUNK_CHARS} of {seen_chars} chars shown live per tick (LIVE_CHUNK_CHARS {LIVE_CHUNK_CHARS});")),
            "{text}"
        );
        assert!(text.contains("the cell's result"), "{text}");
    }

    #[test]
    fn nothing_is_sent_once_the_cell_has_ended() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let out = Arc::new(LiveOutput::new("c1".to_owned(), tx));
        let live = LiveCell {
            out: Some(out.clone()),
            tick: None,
        };
        drop(live);
        out.push("stdout", "late\n");
        out.flush();
        assert!(
            drain(&mut rx).is_empty(),
            "output sent after the cell ended"
        );
    }
}
