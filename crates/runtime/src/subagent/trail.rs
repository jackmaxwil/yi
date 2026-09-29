use std::path::PathBuf;

use yi_types::subagent::{CHILD_ENTRY, ChildEnded, ChildExit, ChildSpawned, ChildTrail};

use crate::session::AgentSession;

impl super::SubagentHost {
    pub(super) fn children_dir(&self) -> PathBuf {
        if self.options.depth > 0 {
            return self.options.parent_session_dir.join("children");
        }
        self.parent_file().map_or_else(
            || self.options.parent_session_dir.clone(),
            |file| file.with_extension("").join("children"),
        )
    }

    fn parent_file(&self) -> Option<PathBuf> {
        let store = (self.options.store)()?;
        yi_session::lock_session(&store).file_path().cloned()
    }

    pub(super) fn parent_id(&self) -> Option<String> {
        let store = (self.options.store)()?;
        Some(yi_session::lock_session(&store).metadata().id.clone())
    }

    pub(super) fn trail_spawned(&self, name: &str, id: &str, child: &AgentSession, brief: &str) {
        let (Some(parent), Some(store)) = (self.parent_file(), child.store()) else {
            return;
        };
        let (file, session) = {
            let store = yi_session::lock_session(&store);
            (store.file_path().cloned(), store.metadata().id.clone())
        };
        let Some(path) = file.and_then(|file| {
            let base = parent.parent()?;
            Some(file.strip_prefix(base).ok()?.to_string_lossy().into_owned())
        }) else {
            return;
        };
        self.journal_trail(&ChildTrail::Spawned(ChildSpawned {
            name: name.to_owned(),
            id: id.to_owned(),
            session,
            path,
            brief: crate::fetch::content_hash(brief),
        }));
    }

    pub(super) fn trail_ended(
        &self,
        name: &str,
        id: &str,
        exit: ChildExit,
        error: Option<String>,
        tokens: u64,
    ) {
        self.journal_trail(&ChildTrail::Ended(ChildEnded {
            name: name.to_owned(),
            id: id.to_owned(),
            exit,
            error,
            tokens,
        }));
    }

    fn journal_trail(&self, line: &ChildTrail) {
        let (Some(store), Ok(data)) = ((self.options.store)(), serde_json::to_value(line)) else {
            return;
        };
        let _a_refused_line_never_blocks_a_child =
            yi_session::lock_session(&store).append_custom("main", CHILD_ENTRY, Some(data));
    }

    pub fn trail(&self, name: &str) -> Option<PathBuf> {
        let base = self.parent_file()?.parent()?.to_path_buf();
        let store = (self.options.store)()?;
        let query = yi_session::EntryQuery {
            custom_type: Some(CHILD_ENTRY.to_owned()),
            ..Default::default()
        };
        let entries = yi_session::lock_session(&store).find_entries(&query).ok()?;
        entries.iter().rev().find_map(|entry| {
            let yi_types::entry::Entry::Custom {
                data: Some(data), ..
            } = entry
            else {
                return None;
            };
            match serde_json::from_value(data.clone()).ok()? {
                ChildTrail::Spawned(line) if line.name == name => Some(base.join(line.path)),
                _ => None,
            }
        })
    }
}
