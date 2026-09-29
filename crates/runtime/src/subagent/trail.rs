use std::path::PathBuf;

use yi_types::subagent::{CHILD_ENTRY, ChildEnded, ChildExit, ChildId, ChildSpawned, ChildTrail};

use crate::session::AgentSession;

impl super::SubagentHost {
    pub(super) fn children_dir(&self) -> PathBuf {
        if self.options.depth > 0 {
            return self.options.parent_session_dir.join("children");
        }
        match self.parent_file() {
            Some(file) if file.extension().is_some_and(|ext| ext == "jsonl") => {
                file.with_extension("").join("children")
            }
            _ => self.options.parent_session_dir.clone(),
        }
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
        journal(
            (self.options.store)().as_ref(),
            &ChildTrail::Spawned(ChildSpawned {
                name: name.to_owned(),
                id: ChildId(id.to_owned()),
                session,
                path,
                brief: crate::fetch::content_hash(brief),
            }),
        );
    }

    pub(super) fn trail_ended(
        store: Option<&yi_session::SharedSession>,
        name: &str,
        id: &str,
        (exit, error, tokens): (ChildExit, Option<String>, u64),
    ) {
        let line = ChildTrail::Ended(ChildEnded {
            name: name.to_owned(),
            id: ChildId(id.to_owned()),
            exit,
            error,
            tokens,
        });
        journal(store, &line);
    }

    pub fn trail(&self, name: &str) -> Option<PathBuf> {
        let base = self.parent_file()?.parent()?.to_path_buf();
        let store = (self.options.store)()?;
        let query = yi_session::EntryQuery {
            custom_type: Some(CHILD_ENTRY.to_owned()),
            ..Default::default()
        };
        let entries = yi_session::lock_session(&store).find_entries(&query).ok()?;
        let suffix = format!("/{name}");
        entries.iter().find_map(|entry| {
            let yi_types::entry::Entry::Custom {
                data: Some(data), ..
            } = entry
            else {
                return None;
            };
            match serde_json::from_value(data.clone()).ok()? {
                ChildTrail::Spawned(line)
                    if line.name == name
                        || line.id.as_str() == name
                        || line.name.ends_with(&suffix) =>
                {
                    Some(base.join(line.path))
                }
                _ => None,
            }
        })
    }
}

fn journal(store: Option<&yi_session::SharedSession>, line: &ChildTrail) {
    let (Some(store), Ok(data)) = (store, serde_json::to_value(line)) else {
        return;
    };
    let _a_refused_line_never_blocks_a_child =
        yi_session::lock_session(store).append_custom("main", CHILD_ENTRY, Some(data));
}
