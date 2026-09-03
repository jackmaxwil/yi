//! Cells the console runs on a session's kernel: what the queue admits, what a cancel
//! reaches, what a close takes down.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tokio::task::JoinHandle;

pub const MAX_CELL_BYTES: usize = 64 * 1024;
pub const MAX_QUEUED_CELLS: usize = 8;
const INVALID_PARAMS: i64 = -32602;
const BUSY: i64 = -32000;

pub struct UserCell {
    pub call_id: String,
    pub cancelled: Arc<AtomicBool>,
    pub task: JoinHandle<()>,
}

/// Invariant: a cell is tracked only while it can still be cancelled, so a full queue is
/// eight cells genuinely still running.
#[derive(Default)]
pub struct UserCells {
    by_session: HashMap<String, Vec<UserCell>>,
    serial: u64,
}

impl UserCells {
    /// Retire finished cells, then mint the next call id or refuse a full queue.
    pub fn admit(&mut self, session: &str) -> Result<String, (i64, String)> {
        let cells = self.by_session.entry(session.to_owned()).or_default();
        cells.retain(|cell| !cell.task.is_finished());
        if cells.len() >= MAX_QUEUED_CELLS {
            return Err((
                BUSY,
                format!("kernel busy: {MAX_QUEUED_CELLS} user cells are queued"),
            ));
        }
        self.serial = self.serial.saturating_add(1);
        Ok(format!("user-{}", self.serial))
    }

    pub fn track(&mut self, session: &str, cell: UserCell) {
        self.by_session
            .entry(session.to_owned())
            .or_default()
            .push(cell);
    }

    pub fn cancel(&mut self, session: &str, call_id: &str) -> bool {
        let Some(cells) = self.by_session.get_mut(session) else {
            return false;
        };
        let Some(cell) = cells.iter().find(|cell| cell.call_id == call_id) else {
            return false;
        };
        cell.cancelled.store(true, Ordering::SeqCst);
        true
    }

    /// The flag first, so a cell past its abort point still stops; then the handle, so
    /// none is left detached.
    pub fn close(&mut self, session: &str) {
        for cell in self.by_session.remove(session).unwrap_or_default() {
            cell.cancelled.store(true, Ordering::SeqCst);
            cell.task.abort();
        }
    }

    pub fn queued(&self, session: &str) -> usize {
        self.by_session.get(session).map_or(0, Vec::len)
    }
}

/// Refused past [`MAX_CELL_BYTES`] before anything spawns: the payload is peer-controlled.
pub fn cell_code(params: &Value) -> Result<String, (i64, String)> {
    let code = params
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if code.len() > MAX_CELL_BYTES {
        return Err((
            INVALID_PARAMS,
            format!("a cell is at most {} KB", MAX_CELL_BYTES / 1024),
        ));
    }
    Ok(code.to_owned())
}
