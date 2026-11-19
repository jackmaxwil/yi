//! What the agent did to each file, read from the thread's tool calls.

use std::collections::BTreeMap;

use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::ToolState;

/// One file's marks: how often it was read, whether it was edited, whether a write runs now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mark {
    pub reads: u32,
    pub edited: bool,
    pub writing: bool,
}

impl Mark {
    /// The word beside a file read but not edited: "read", "read twice", "read 3 times".
    pub fn word(&self) -> Option<String> {
        match (self.edited || self.writing, self.reads) {
            (true, _) | (_, 0) => None,
            (_, 1) => Some("read".to_owned()),
            (_, 2) => Some("read twice".to_owned()),
            (_, n) => Some(format!("read {n} times")),
        }
    }
}

/// Each path the thread's `read`, `edit` and `write` calls named, relative to `root`, the
/// folder it works in.
pub fn marks(thread: &Thread, root: &str) -> BTreeMap<String, Mark> {
    let mut marks = BTreeMap::<String, Mark>::new();
    for call in thread.transcript.tools.values() {
        let Some(path) = call.args.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let (read, edit) = (
            call.name == "read",
            matches!(call.name.as_str(), "edit" | "write"),
        );
        if !read && !edit {
            continue;
        }
        let path = path
            .strip_prefix(root)
            .map_or(path, |p| p.trim_start_matches('/'));
        let mark = marks
            .entry(path.trim_start_matches("./").to_owned())
            .or_default();
        if read {
            mark.reads = mark.reads.saturating_add(1);
        } else {
            mark.edited = true;
            mark.writing |= matches!(call.state, ToolState::Running | ToolState::Asking);
        }
    }
    marks
}
