//! The show bridge: a session's `show` call reaches the attached client as a `_yi/show`
//! request, and the client's answer, or a short wait with none, goes back to the tool.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use yi_runtime::{ShowAsk, Shower};
use yi_types::acp::{AcpFrame, AcpShowParams, AcpShowResult, SHOW_METHOD};

use crate::permission::PendingAsks;
use crate::{AcpState, LineSink};

const POLL: Duration = Duration::from_millis(100);
/// A show never holds a turn: with no answer after this many polls, five seconds, the model
/// is told no display answered and carries on.
pub const SHOW_POLLS: u32 = 50;

/// Written before blocking and answered by the stdin reader thread, as a permission ask is,
/// so the executor never waits on itself. No answer, an error or an abort is `None`.
pub fn bridge_shower(
    session_id: String,
    sink: LineSink,
    pending: PendingAsks,
    polls: u32,
) -> Shower {
    let counter = AtomicU64::new(0);
    Arc::new(move |ask: &ShowAsk<'_>| {
        let n = counter.fetch_add(1, Ordering::Relaxed);
        let id = format!("show_{session_id}_{n}");
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut asks = pending.lock().ok()?;
        asks.as_mut()?.insert(id.clone(), sender);
        drop(asks);
        let params = AcpShowParams {
            session_id: session_id.clone(),
            address: ask.address.to_owned(),
            cwd: ask.cwd.to_string_lossy().into_owned(),
            strength: ask.strength.clone(),
        };
        let request = AcpFrame {
            jsonrpc: "2.0".to_owned(),
            id: Some(Value::String(id.clone())),
            method: SHOW_METHOD.to_owned(),
            params: serde_json::to_value(&params).ok(),
        };
        sink(&json!(request));
        let stopped = || ask.stop.is_some_and(|stop| stop());
        let mut answer = None;
        for _ in 0..polls {
            answer = receiver.recv_timeout(POLL).ok();
            if answer.is_some() || stopped() {
                break;
            }
        }
        if let Some(asks) = pending.lock().ok().as_mut().and_then(|asks| asks.as_mut()) {
            asks.remove(&id);
        }
        let result = answer?.get("result").cloned()?;
        serde_json::from_value::<AcpShowResult>(result).ok()
    })
}

impl AcpState {
    /// Notes that the client opening `session_id` said, in `_meta.yi.shows`, that it can show
    /// things. Read when the session is built: its tool set is frozen from then on.
    pub(crate) fn note_shows(&mut self, session_id: &str, params: &Value) {
        let shows = params.pointer("/_meta/yi/shows").and_then(Value::as_bool);
        if shows == Some(true) {
            self.shows.insert(session_id.to_owned());
        }
    }

    /// The session a resume names, closed no longer, with its client's word on showing noted.
    pub(crate) fn reopened(&mut self, params: &Value) -> Result<String, (i64, String)> {
        let id = Self::session_id(params)?;
        self.closed.remove(&id);
        self.note_shows(&id, params);
        Ok(id)
    }

    pub(crate) fn shower(&self, session_id: &str) -> Option<Shower> {
        let (sink, pending) = (Arc::clone(&self.sink), Arc::clone(&self.pending));
        let known = self.shows.contains(session_id);
        known.then(|| bridge_shower(session_id.to_owned(), sink, pending, SHOW_POLLS))
    }
}
