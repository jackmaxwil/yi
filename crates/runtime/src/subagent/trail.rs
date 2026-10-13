use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

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
        let store = yi_session::lock_session(&store);
        spawned_path(&store, &base, name)
    }
}

/// The file of the child `name` (or id) that `store`'s trail records as spawned, under `base`.
pub(crate) fn spawned_path(
    store: &yi_session::SessionStore,
    base: &std::path::Path,
    name: &str,
) -> Option<PathBuf> {
    let query = yi_session::EntryQuery {
        custom_type: Some(CHILD_ENTRY.to_owned()),
        ..Default::default()
    };
    store.find_entries(&query).ok()?.iter().find_map(|entry| {
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

fn journal(store: &yi_session::SharedSession, line: &ChildTrail) {
    let Ok(data) = serde_json::to_value(line) else {
        return;
    };
    let _a_refused_line_never_blocks_a_child =
        yi_session::lock_session(store).append_custom("main", CHILD_ENTRY, Some(data));
}

/// Reaped children's transcripts held open; a hundred-child swarm pinned every store before.
const KEPT_OPEN: usize = 4;

/// Invariant: at most one live store per reaped child, so an inbox write and a history read
/// never append through two instances of one file.
#[derive(Default)]
pub(crate) struct Kept(Vec<KeptChild>);

struct KeptChild {
    name: String,
    file: Option<PathBuf>,
    live: Weak<Mutex<yi_session::SessionStore>>,
    held: Option<yi_session::SharedSession>,
}

impl Kept {
    pub(crate) fn keep(&mut self, name: String, store: yi_session::SharedSession) {
        let file = yi_session::lock_session(&store).file_path().cloned();
        self.0.retain(|child| child.name != name);
        let live = Arc::downgrade(&store);
        self.0.push(KeptChild {
            name,
            file,
            live,
            held: Some(store),
        });
        self.trim();
    }

    /// Most recently used children sit last; a file-backed one past the cap lets its store go.
    fn trim(&mut self) {
        let mut open = 0;
        for child in self.0.iter_mut().rev().filter(|child| child.file.is_some()) {
            open += usize::from(child.held.is_some());
            if open > KEPT_OPEN {
                child.held = None;
            }
        }
    }

    pub(crate) fn get(&mut self, name: &str) -> Option<yi_session::SharedSession> {
        let at = self.0.iter().position(|child| child.name == name)?;
        let mut child = self.0.remove(at);
        let store = child.live.upgrade().or_else(|| {
            let file = child.file.as_ref()?;
            Some(Arc::new(Mutex::new(yi_session::load_session(file).ok()?)))
        });
        if let Some(store) = &store {
            child.live = Arc::downgrade(store);
            child.held = Some(Arc::clone(store));
        }
        self.0.push(child);
        self.trim();
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Outcome = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn kept_children_past_the_cap_reopen_once_from_their_file() -> Outcome {
        let root = crate::scratch::Scratch::new("yi-kept-transcripts")?;
        let mut repo = yi_session::JsonlRepo::new(root.to_path_buf(), "/work");
        let mut kept = Kept::default();
        let mut first = Weak::new();
        for n in 0..=KEPT_OPEN {
            let options = yi_session::CreateOptions {
                id: None,
                parent_session_id: None,
                metadata: None,
            };
            let store = yi_session::SessionRepo::create(&mut repo, options)?;
            if n == 0 {
                first = Arc::downgrade(&store);
            }
            kept.keep(format!("child-{n}"), store);
        }
        assert!(
            first.upgrade().is_none(),
            "a child past the cap stayed resident"
        );
        let reopened = kept.get("child-0").ok_or("child-0 not reopened")?;
        let again = kept.get("child-0").ok_or("child-0 lost")?;
        assert!(
            Arc::ptr_eq(&reopened, &again),
            "two live stores for one file"
        );
        yi_session::lock_session(&reopened).append_custom("main", "inbox", None)?;
        yi_session::lock_session(&again).append_custom("main", "inbox", None)?;
        let file = yi_session::lock_session(&again)
            .file_path()
            .cloned()
            .ok_or("no file")?;
        let query = yi_session::EntryQuery {
            custom_type: Some("inbox".to_owned()),
            ..Default::default()
        };
        let reloaded = yi_session::load_session(&file)?;
        assert_eq!(reloaded.find_entries(&query)?.len(), 2);
        Ok(())
    }
}
