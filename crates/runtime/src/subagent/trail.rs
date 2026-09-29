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

    pub(super) fn trail_spawned(
        &self,
        name: &str,
        id: &str,
        child: &AgentSession,
        brief: &str,
    ) -> Option<PathBuf> {
        let parent = (self.options.store)()?;
        let parent_file = yi_session::lock_session(&parent).file_path().cloned()?;
        let (file, session) = {
            let store = child.store()?;
            let store = yi_session::lock_session(&store);
            (store.file_path().cloned()?, store.metadata().id.clone())
        };
        let path = file.strip_prefix(parent_file.parent()?).ok()?;
        journal(
            &parent,
            &ChildTrail::Spawned(ChildSpawned {
                name: name.to_owned(),
                id: ChildId(id.to_owned()),
                session,
                path: path.to_string_lossy().into_owned(),
                brief: crate::fetch::content_hash(brief),
            }),
        );
        Some(parent_file)
    }

    pub(super) fn trail_ended(
        &self,
        spawned_in: Option<&PathBuf>,
        name: &str,
        id: &str,
        (exit, error, tokens): (ChildExit, Option<String>, u64),
    ) {
        let Some(store) = (self.options.store)() else {
            return;
        };
        if spawned_in.is_none() || yi_session::lock_session(&store).file_path() != spawned_in {
            return;
        }
        let line = ChildTrail::Ended(ChildEnded {
            name: name.to_owned(),
            id: ChildId(id.to_owned()),
            exit,
            error,
            tokens,
        });
        journal(&store, &line);
    }

    pub fn trail(&self, name: &str) -> Option<PathBuf> {
        let base = self.parent_file()?.parent()?.to_path_buf();
        let store = (self.options.store)()?;
        let query = yi_session::EntryQuery {
            custom_type: Some(CHILD_ENTRY.to_owned()),
            ..Default::default()
        };
        let entries = yi_session::lock_session(&store).find_entries(&query).ok()?;
        entries.iter().find_map(|entry| {
            let yi_types::entry::Entry::Custom {
                data: Some(data), ..
            } = entry
            else {
                return None;
            };
            match serde_json::from_value(data.clone()).ok()? {
                ChildTrail::Spawned(line) if line.name == name || line.id.as_str() == name => {
                    Some(base.join(line.path))
                }
                _ => None,
            }
        })
    }
}

fn journal(store: &yi_session::SharedSession, line: &ChildTrail) {
    let Ok(data) = serde_json::to_value(line) else {
        return;
    };
    let _a_refused_line_never_blocks_a_child =
        yi_session::lock_session(store).append_custom("main", CHILD_ENTRY, Some(data));
}
