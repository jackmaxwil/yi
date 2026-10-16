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
pub(crate) struct Kept(Mutex<Vec<KeptChild>>);

struct KeptChild {
    name: String,
    file: Option<PathBuf>,
    live: Weak<Mutex<yi_session::SessionStore>>,
    held: Option<yi_session::SharedSession>,
}

impl Kept {
    pub(crate) fn keep(&self, name: String, store: yi_session::SharedSession) {
        let file = yi_session::lock_session(&store).file_path().cloned();
        let held = file.is_none().then(|| Arc::clone(&store));
        let live = Arc::downgrade(&store);
        let Ok(mut children) = self.0.lock() else {
            return;
        };
        children.retain(|child| child.name != name);
        children.push(KeptChild {
            name,
            file,
            live,
            held,
        });
        trim(&mut children);
    }

    /// Reaping pins no file-backed child; a read reopens it under this lock, and then it stays,
    /// most recently used last, until the cap releases the oldest.
    pub(crate) fn get(&self, name: &str) -> Option<yi_session::SharedSession> {
        let mut children = self.0.lock().ok()?;
        let at = children.iter().position(|child| child.name == name)?;
        let child = children.get_mut(at)?;
        let store = if let Some(store) = child.held.clone() {
            store
        } else if let Some(store) = child.live.upgrade() {
            store
        } else {
            let file = child.file.clone()?;
            let store = yi_session::open_shared(&file)?;
            child.live = Arc::downgrade(&store);
            store
        };
        child.held = Some(Arc::clone(&store));
        let child = children.remove(at);
        children.push(child);
        trim(&mut children);
        Some(store)
    }
}

/// Most recently used children sit last; a file-backed one past the cap lets its store go.
fn trim(children: &mut [KeptChild]) {
    let mut open = 0;
    for child in children
        .iter_mut()
        .rev()
        .filter(|child| child.file.is_some())
    {
        open += usize::from(child.held.is_some());
        if open > KEPT_OPEN {
            child.held = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Outcome = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn kept_children_are_not_pinned_and_reopen_once_from_their_file() -> Outcome {
        let root = crate::scratch::Scratch::new("yi-kept-transcripts")?;
        let mut repo = yi_session::JsonlRepo::new(root.to_path_buf(), "/work");
        let kept = Kept::default();
        let mut first = Weak::new();
        let mut last = Weak::new();
        for n in 0..5 {
            let options = yi_session::CreateOptions {
                id: None,
                parent_session_id: None,
                metadata: None,
            };
            let store = yi_session::SessionRepo::create(&mut repo, options)?;
            if n == 0 {
                first = Arc::downgrade(&store);
            }
            if n == 4 {
                last = Arc::downgrade(&store);
            }
            kept.keep(format!("child-{n}"), store);
        }
        assert!(
            first.upgrade().is_none(),
            "a kept child's store stayed resident with no reader"
        );
        assert!(
            last.upgrade().is_none(),
            "the most recently reaped child's store stayed resident with no reader"
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

    #[test]
    fn kept_open_children_release_the_oldest_past_the_cap() -> Outcome {
        let root = crate::scratch::Scratch::new("yi-kept-cap")?;
        let mut repo = yi_session::JsonlRepo::new(root.to_path_buf(), "/work");
        let kept = Kept::default();
        for n in 0..5 {
            let options = yi_session::CreateOptions {
                id: None,
                parent_session_id: None,
                metadata: None,
            };
            let store = yi_session::SessionRepo::create(&mut repo, options)?;
            kept.keep(format!("child-{n}"), store);
        }
        let oldest = Arc::downgrade(&kept.get("child-0").ok_or("child-0 not reopened")?);
        for n in 1..=4 {
            kept.get(&format!("child-{n}")).ok_or("not reopened")?;
        }
        assert!(
            oldest.upgrade().is_none(),
            "a file past the cap stayed held"
        );
        let newest = Arc::downgrade(&kept.get("child-4").ok_or("child-4 not reopened")?);
        assert!(
            newest.upgrade().is_some(),
            "the most recent child was released"
        );
        Ok(())
    }

    #[test]
    fn a_failed_reopen_keeps_the_child_for_the_next_read() -> Outcome {
        let root = crate::scratch::Scratch::new("yi-kept-reopen")?;
        let mut repo = yi_session::JsonlRepo::new(root.to_path_buf(), "/work");
        let kept = Kept::default();
        let options = yi_session::CreateOptions {
            id: None,
            parent_session_id: None,
            metadata: None,
        };
        let store = yi_session::SessionRepo::create(&mut repo, options)?;
        yi_session::lock_session(&store).append_custom("main", "inbox", None)?;
        let file = yi_session::lock_session(&store)
            .file_path()
            .cloned()
            .ok_or("no file")?;
        kept.keep("child".to_owned(), store);
        let aside = file.with_extension("aside");
        std::fs::rename(&file, &aside)?;
        assert!(kept.get("child").is_none(), "a missing file reopened");
        std::fs::rename(&aside, &file)?;
        assert!(
            kept.get("child").is_some(),
            "a failed reopen dropped the child"
        );
        Ok(())
    }
}
