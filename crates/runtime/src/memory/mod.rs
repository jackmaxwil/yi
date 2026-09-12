mod doc;
mod ext;
mod store;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

pub use doc::Note;
use doc::Scope;
pub use ext::{MemoryExt, block};
pub use store::{Store, global_dir, repo_dir};

const SAVE_CAP: usize = 3;

type Reply = Result<Map<String, Value>, String>;

#[derive(Debug, Default)]
pub struct Activity {
    state: Mutex<(Vec<String>, usize)>,
}

impl Activity {
    fn push(&self, line: String) {
        if let Ok(mut state) = self.state.lock() {
            state.0.push(line);
        }
    }

    fn saved(&self, line: String) {
        if let Ok(mut state) = self.state.lock() {
            state.0.push(line);
            state.1 = state.1.saturating_add(1);
        }
    }

    pub fn take_lines(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|mut state| std::mem::take(&mut state.0))
            .unwrap_or_default()
    }

    pub fn hud(&self) -> Option<String> {
        let saved = self.state.lock().map(|state| state.1).unwrap_or(0);
        (saved > 0).then(|| format!("saved {saved}"))
    }
}

struct Verbs {
    home: PathBuf,
    cwd: PathBuf,
    root: bool,
    saves: AtomicUsize,
    activity: Arc<Activity>,
}

fn text(payload: &Map<String, Value>, key: &str, verb: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{verb}: {key} must be a string"))
}

fn scope_of(payload: &Map<String, Value>, verb: &str) -> Result<Vec<Scope>, String> {
    match payload.get("scope").and_then(Value::as_str) {
        None => Ok(vec![Scope::Repo, Scope::Global]),
        Some(raw) => Scope::parse(raw)
            .map(|scope| vec![scope])
            .ok_or_else(|| format!("{verb}: scope is {raw:?}; it must be repo or global")),
    }
}

impl Verbs {
    fn store(&self, scope: Scope) -> Store {
        Store::new(match scope {
            Scope::Repo => repo_dir(&self.home, &self.cwd),
            Scope::Global => global_dir(&self.home),
        })
    }

    fn root_only(&self, verb: &str) -> Result<(), String> {
        if self.root {
            Ok(())
        } else {
            Err(format!(
                "{verb}: only the root session keeps memory; the parent puts what a child needs in its task"
            ))
        }
    }

    fn save(&self, payload: &Map<String, Value>) -> Reply {
        self.root_only("memory.save")?;
        let reserved = self
            .saves
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < SAVE_CAP).then_some(n.saturating_add(1))
            });
        if let Err(spent) = reserved {
            return Err(format!(
                "memory.save: at most {SAVE_CAP} saves per session; this is save {}",
                spent.saturating_add(1)
            ));
        }
        let saved = self.save_reserved(payload);
        if saved.is_err() {
            self.saves.fetch_sub(1, Ordering::Relaxed);
        }
        saved
    }

    fn save_reserved(&self, payload: &Map<String, Value>) -> Reply {
        let markdown = text(payload, "markdown", "memory.save")?;
        let mut overlay = Vec::new();
        if let Some(fields) = payload.get("fields").and_then(Value::as_object) {
            for (key, value) in fields {
                let value = value
                    .as_str()
                    .ok_or_else(|| format!("memory.save: {key} must be a string"))?;
                overlay.push((key.clone(), value.to_owned()));
            }
        }
        let draft =
            doc::draft(&markdown, &overlay).map_err(|error| format!("memory.save: {error}"))?;
        let memory = draft.memory;
        let updated = self
            .store(draft.scope)
            .save(memory.clone())
            .map_err(|error| format!("memory.save: {error}"))?;
        let mut line = format!(
            "memory · saved {} · {} — {}",
            memory.name,
            draft.scope.as_str(),
            memory.hook
        );
        for warning in &draft.warnings {
            line.push_str(&format!(" · {warning}"));
        }
        self.activity.saved(line);
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::from(memory.name.as_str()));
        reply.insert("description".to_owned(), Value::from(memory.hook));
        reply.insert("type".to_owned(), Value::from(memory.kind.as_str()));
        reply.insert("scope".to_owned(), Value::from(draft.scope.as_str()));
        reply.insert("updated".to_owned(), Value::Bool(updated));
        reply.insert("warnings".to_owned(), Value::from(draft.warnings));
        Ok(reply)
    }

    fn find(&self, payload: &Map<String, Value>, verb: &str) -> Result<(Scope, Note), String> {
        let query = text(payload, "name", verb)?;
        scope_of(payload, verb)?
            .into_iter()
            .find_map(|scope| self.store(scope).resolve(&query).map(|note| (scope, note)))
            .ok_or_else(|| format!("{verb}: no note matches {query:?} by name or hook"))
    }

    fn read(&self, payload: &Map<String, Value>) -> Reply {
        self.root_only("memory.read")?;
        let (scope, note) = self.find(payload, "memory.read")?;
        let mut warnings: Vec<String> = note
            .trouble
            .iter()
            .map(|trouble| format!("{trouble}; indexed by its first line"))
            .collect();
        let counted = self.store(scope).record(|usage| {
            let entry = usage.notes.entry(note.name.to_string()).or_default();
            entry.reads = entry.reads.saturating_add(1);
            entry.last = store::now();
        });
        if let Err(error) = counted {
            warnings.push(format!("the read was not counted: {error}"));
        }
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::from(note.name.as_str()));
        reply.insert("description".to_owned(), Value::from(note.hook));
        reply.insert(
            "type".to_owned(),
            note.kind
                .map_or(Value::Null, |kind| Value::from(kind.as_str())),
        );
        reply.insert("scope".to_owned(), Value::from(scope.as_str()));
        reply.insert("text".to_owned(), Value::from(note.body));
        reply.insert("warnings".to_owned(), Value::from(warnings));
        Ok(reply)
    }

    fn forget(&self, payload: &Map<String, Value>) -> Reply {
        self.root_only("memory.forget")?;
        let (scope, note) = self.find(payload, "memory.forget")?;
        self.store(scope)
            .forget(&note.name)
            .map_err(|error| format!("memory.forget: {error}"))?;
        self.activity.push(format!(
            "memory · forgot {} · {}",
            note.name,
            scope.as_str()
        ));
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::from(note.name.as_str()));
        reply.insert("scope".to_owned(), Value::from(scope.as_str()));
        Ok(reply)
    }
}

fn register_verb(
    registry: &mut crate::kernel::HostRegistry,
    verbs: &Arc<Verbs>,
    name: &'static str,
    run: fn(&Verbs, &Map<String, Value>) -> Reply,
) {
    let verbs = Arc::clone(verbs);
    registry.register(name, move |payload| {
        let verbs = Arc::clone(&verbs);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&verbs, &payload))
                .await
                .map_err(|error| format!("{name}: {error}"))?
        })
    });
}

pub fn attach(
    session: Option<&crate::AgentSession>,
    registry: &mut crate::kernel::HostRegistry,
    home: PathBuf,
    cwd: PathBuf,
    root: bool,
) {
    let activity = Arc::new(Activity::default());
    if let Some(session) = session.filter(|_| root) {
        if let Some(host) = session.extensions()
            && let Ok(mut host) = host.lock()
        {
            host.register(Box::new(MemoryExt::new(
                home.clone(),
                Arc::clone(&activity),
            )));
        }
        session.set_memory(Arc::clone(&activity));
    }
    let verbs = Arc::new(Verbs {
        home,
        cwd,
        root,
        saves: AtomicUsize::new(0),
        activity,
    });
    register_verb(registry, &verbs, "memory.save", Verbs::save);
    register_verb(registry, &verbs, "memory.read", Verbs::read);
    register_verb(registry, &verbs, "memory.forget", Verbs::forget);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs(label: &str, root: bool) -> Verbs {
        let base = std::env::temp_dir().join(format!("yi-memverbs-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("cwd")).unwrap();
        Verbs {
            home: base.join("home"),
            cwd: base.join("cwd"),
            root,
            saves: AtomicUsize::new(0),
            activity: Arc::new(Activity::default()),
        }
    }

    fn save_payload(name: &str) -> Map<String, Value> {
        let markdown =
            format!("---\nname: {name}\ndescription: hook for {name}\ntype: project\n---\nbody\n");
        let mut payload = Map::new();
        payload.insert("markdown".to_owned(), Value::from(markdown));
        payload
    }

    #[test]
    fn a_fourth_save_is_refused_and_a_refused_save_is_refunded() {
        let verbs = verbs("cap", true);
        let mut bad = Map::new();
        bad.insert("markdown".to_owned(), Value::from("no type here"));
        let err = verbs.save(&bad).unwrap_err();
        assert!(err.starts_with("memory.save: no type"), "{err}");
        for name in ["one", "two", "three"] {
            verbs.save(&save_payload(name)).unwrap();
        }
        let err = verbs.save(&save_payload("four")).unwrap_err();
        assert_eq!(
            err,
            "memory.save: at most 3 saves per session; this is save 4"
        );
        assert_eq!(verbs.activity.hud().as_deref(), Some("saved 3"));
        assert_eq!(verbs.activity.take_lines().len(), 3);
        assert!(verbs.activity.take_lines().is_empty());
    }

    #[test]
    fn a_child_is_refused_every_verb() {
        let verbs = verbs("child", false);
        let err = verbs.save(&save_payload("x")).unwrap_err();
        assert!(err.contains("only the root session"), "{err}");
        let mut name = Map::new();
        name.insert("name".to_owned(), Value::from("x"));
        assert!(verbs.read(&name).is_err());
        assert!(verbs.forget(&name).is_err());
    }

    #[test]
    fn read_counts_and_replies_without_a_path() {
        let verbs = verbs("read", true);
        verbs.save(&save_payload("alpha")).unwrap();
        let mut query = Map::new();
        query.insert("name".to_owned(), Value::from("hook for alpha"));
        let reply = verbs.read(&query).unwrap();
        assert_eq!(reply["name"], "alpha");
        assert_eq!(reply["text"], "body");
        let dumped = serde_json::to_string(&reply).unwrap();
        assert!(!dumped.contains(".yi"), "{dumped}");
        let usage = verbs.store(Scope::Repo).usage();
        assert_eq!(usage.notes.get("alpha").map(|entry| entry.reads), Some(1));
        verbs.forget(&query).unwrap();
        assert!(verbs.read(&query).is_err());
    }
}
