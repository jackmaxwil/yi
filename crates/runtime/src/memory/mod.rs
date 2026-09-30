mod doc;
mod ext;
mod journal;
pub(crate) mod rank;
mod store;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::args::Args;
use serde_json::{Map, Value};
use yi_types::memory::MemoryPointer;
use yi_types::plan::canonical::Digest;

pub use doc::Note;
use doc::Scope;
pub use ext::{MemoryExt, block};
pub use rank::tokens;
pub use store::{Store, global_dir, repo_dir};

const SEARCH_LIMIT: usize = 5;

pub fn ranked(stores: &[&Store], query: &str) -> Vec<(usize, Note)> {
    let notes: Vec<(usize, Note)> = stores
        .iter()
        .enumerate()
        .flat_map(|(at, store)| store.notes().into_iter().map(move |note| (at, note)))
        .collect();
    let texts: Vec<String> = notes
        .iter()
        .map(|(_, note)| format!("{} {} {}", note.name, note.hook, note.body))
        .collect();
    let index = rank::Bm25::new(texts.iter().map(String::as_str));
    index
        .rank(query)
        .into_iter()
        .filter_map(|(at, _)| notes.get(at).cloned())
        .collect()
}

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
    session: Option<crate::goal::StoreHandle>,
}

fn text(payload: &Map<String, Value>, key: &str, verb: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{verb}: {key} must be a string"))
}

fn scope_of(payload: &Map<String, Value>, verb: &str) -> Result<Vec<Scope>, String> {
    match payload.str_of("scope") {
        None => Ok(vec![Scope::Repo, Scope::Global]),
        Some(raw) => Scope::parse(raw)
            .map(|scope| vec![scope])
            .ok_or_else(|| format!("{verb}: scope is {raw:?}; it must be repo or global")),
    }
}

fn search_call(query: &str, payload: &Map<String, Value>, limit: usize) -> String {
    let scope = payload
        .str_of("scope")
        .map_or_else(String::new, |raw| format!(", scope={}", Value::from(raw)));
    format!(
        "memory.search({}{scope}, limit={limit})",
        Value::from(query)
    )
}

impl Verbs {
    fn store(&self, scope: Scope) -> Store {
        let session = self
            .session
            .as_ref()
            .and_then(|handle| handle())
            .map(|shared| yi_session::lock_session(&shared).metadata().id.clone());
        Store::new(match scope {
            Scope::Repo => repo_dir(&self.home, &self.cwd),
            Scope::Global => global_dir(&self.home),
        })
        .with_session(session)
    }

    fn point(&self, op: &str, name: &str, scope: Scope, hash: Digest) -> Option<String> {
        let shared = self.session.as_ref().and_then(|handle| handle())?;
        let pointer = MemoryPointer {
            op: op.to_owned(),
            name: name.to_owned(),
            scope: scope.as_str().to_owned(),
            hash,
            extra: Map::new(),
        };
        yi_session::lock_session(&shared)
            .append_custom_record(&pointer)
            .err()
            .map(|error| format!("the session did not record the {op}: {error}"))
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
        let saved = self
            .store(draft.scope)
            .save(memory.clone())
            .map_err(|error| format!("memory.save: {error}"))?;
        let mut warnings = draft.warnings;
        warnings.extend(self.point("save", memory.name.as_str(), draft.scope, saved.hash));
        let mut line = format!(
            "memory · saved {} · {} — {}",
            memory.name,
            draft.scope.as_str(),
            memory.hook
        );
        for warning in &warnings {
            line.push_str(&format!(" · {warning}"));
        }
        self.activity.saved(line);
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::from(memory.name.as_str()));
        reply.insert("description".to_owned(), Value::from(memory.hook));
        reply.insert("type".to_owned(), Value::from(memory.kind.as_str()));
        reply.insert("scope".to_owned(), Value::from(draft.scope.as_str()));
        reply.insert("updated".to_owned(), Value::Bool(saved.updated));
        reply.insert("warnings".to_owned(), Value::from(warnings));
        Ok(reply)
    }

    fn search(&self, payload: &Map<String, Value>) -> Reply {
        self.root_only("memory.search")?;
        let query = text(payload, "query", "memory.search")?;
        let limit = payload
            .u64_of("limit")
            .map_or(SEARCH_LIMIT, |n| usize::try_from(n).unwrap_or(SEARCH_LIMIT))
            .max(1);
        let scopes = scope_of(payload, "memory.search")?;
        let stores: Vec<Store> = scopes.iter().map(|scope| self.store(*scope)).collect();
        let hits = ranked(&stores.iter().collect::<Vec<_>>(), &query);
        let total = hits.len();
        let listed: Vec<Value> = hits
            .into_iter()
            .take(limit)
            .filter_map(|(at, note)| {
                let scope = scopes.get(at)?;
                let mut item = Map::new();
                item.insert("name".to_owned(), Value::from(note.name.as_str()));
                item.insert("description".to_owned(), Value::from(note.hook));
                item.insert("scope".to_owned(), Value::from(scope.as_str()));
                Some(Value::Object(item))
            })
            .collect();
        let mut reply = Map::new();
        if listed.len() < total {
            reply.insert(
                "notice".to_owned(),
                Value::from(format!(
                    "[{} of {total} notes · limit {limit} · {} for all]",
                    listed.len(),
                    search_call(&query, payload, total)
                )),
            );
        }
        reply.insert("hits".to_owned(), Value::from(listed));
        reply.insert("total".to_owned(), Value::from(total));
        Ok(reply)
    }

    fn closest(&self, payload: &Map<String, Value>) -> Option<(Scope, Note, String)> {
        let query = payload.str_of("name")?;
        let scopes = scope_of(payload, "memory.read").ok()?;
        let stores: Vec<Store> = scopes.iter().map(|scope| self.store(*scope)).collect();
        let hits = ranked(&stores.iter().collect::<Vec<_>>(), query);
        let total = hits.len();
        let (at, note) = hits.into_iter().next()?;
        let row = format!(
            "[no note has that name or hook · opened {}, the closest of {total} by memory.search · {} for the ranking]",
            note.name,
            search_call(query, payload, total)
        );
        Some((*scopes.get(at)?, note, row))
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
        let (scope, note, searched) = match self.find(payload, "memory.read") {
            Ok((scope, note)) => (scope, note, None),
            Err(missed) => match self.closest(payload) {
                Some((scope, note, row)) => (scope, note, Some(row)),
                None => return Err(missed),
            },
        };
        let mut warnings: Vec<String> = note
            .trouble
            .iter()
            .map(|trouble| format!("{trouble}; indexed by its first line"))
            .collect();
        warnings.extend(searched);
        match self.store(scope).mark_read(&note.name) {
            Ok(hash) => warnings.extend(self.point("read", note.name.as_str(), scope, hash)),
            Err(error) => warnings.push(format!("the read was not counted: {error}")),
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
        let hash = self
            .store(scope)
            .forget(&note.name)
            .map_err(|error| format!("memory.forget: {error}"))?;
        let warnings: Vec<String> = self
            .point("forget", note.name.as_str(), scope, hash)
            .into_iter()
            .collect();
        self.activity.push(format!(
            "memory · forgot {} · {}",
            note.name,
            scope.as_str()
        ));
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::from(note.name.as_str()));
        reply.insert("scope".to_owned(), Value::from(scope.as_str()));
        reply.insert("warnings".to_owned(), Value::from(warnings));
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
        session: session.map(|session| session.store_handle()),
    });
    register_verb(registry, &verbs, "memory.save", Verbs::save);
    register_verb(registry, &verbs, "memory.read", Verbs::read);
    register_verb(registry, &verbs, "memory.forget", Verbs::forget);
    register_verb(registry, &verbs, "memory.search", Verbs::search);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    fn verbs(label: &str, root: bool) -> (Scratch, Verbs) {
        let base = Scratch::new(&format!("yi-memverbs-{label}")).unwrap();
        std::fs::create_dir_all(base.join("cwd")).unwrap();
        let verbs = Verbs {
            home: base.join("home"),
            cwd: base.join("cwd"),
            root,
            saves: AtomicUsize::new(0),
            activity: Arc::new(Activity::default()),
            session: None,
        };
        (base, verbs)
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
        let (_base, verbs) = verbs("cap", true);
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
    fn the_session_records_each_op_by_name_and_hash_and_never_the_body() {
        let (_base, mut verbs) = verbs("pointer", true);
        let shared: yi_session::SharedSession = Arc::new(Mutex::new(
            yi_session::SessionStore::in_memory(yi_types::wire::SessionMetadata {
                id: "s1".to_owned(),
                created_at: 0,
                parent_session_id: None,
                name: None,
            }),
        ));
        let handle = Arc::clone(&shared);
        verbs.session = Some(Arc::new(move || Some(Arc::clone(&handle))));
        verbs.save(&save_payload("alpha")).unwrap();
        let mut query = Map::new();
        query.insert("name".to_owned(), Value::from("alpha"));
        verbs.read(&query).unwrap();
        verbs.forget(&query).unwrap();
        let entries = yi_session::lock_session(&shared)
            .find_entries(&yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            })
            .unwrap();
        let pointers: Vec<MemoryPointer> = yi_session::lock_session(&shared)
            .custom_records(yi_session::EntryOrder::OldestFirst, None);
        let ops: Vec<&str> = pointers.iter().map(|pointer| pointer.op.as_str()).collect();
        assert_eq!(ops, vec!["save", "read", "forget"]);
        assert!(
            pointers
                .iter()
                .all(|pointer| pointer.scope == "repo" && pointer.name == "alpha")
        );
        let dumped = serde_json::to_string(&entries).unwrap();
        assert!(!dumped.contains("hook for alpha"), "{dumped}");
        let replay = verbs.store(Scope::Repo).journal_sessions();
        assert_eq!(replay, vec![Some("s1".to_owned()); 3]);
    }

    fn seed(verbs: &Verbs, name: &str, hook: &str, body: &str) {
        let text = format!("---\nname: {name}\ndescription: {hook}\ntype: feedback\n---\n{body}\n");
        let memory = doc::draft(&text, &[]).unwrap().memory;
        verbs.store(Scope::Repo).save(memory).unwrap();
    }

    #[test]
    fn search_ranks_notes_by_their_words_and_names_its_cap() {
        let (_base, verbs) = verbs("search", true);
        seed(
            &verbs,
            "never-git-reset",
            "never git reset in the shared tree",
            "sync main another way",
        );
        for i in 0..6 {
            seed(
                &verbs,
                &format!("main-{i}"),
                "a note that says main",
                "main",
            );
        }
        let mut query = Map::new();
        query.insert("query".to_owned(), Value::from("sync main"));
        let reply = verbs.search(&query).unwrap();
        assert_eq!(reply["hits"][0]["name"], "never-git-reset");
        assert_eq!(reply["hits"].as_array().map(Vec::len), Some(5));
        assert_eq!(reply["total"], 7);
        assert_eq!(
            reply["notice"],
            "[5 of 7 notes · limit 5 · memory.search(\"sync main\", limit=7) for all]"
        );
        query.insert("scope".to_owned(), Value::from("repo"));
        assert_eq!(
            verbs.search(&query).unwrap()["notice"],
            "[5 of 7 notes · limit 5 · memory.search(\"sync main\", scope=\"repo\", limit=7) for all]"
        );
        query.insert("limit".to_owned(), Value::from(7));
        assert!(verbs.search(&query).unwrap().get("notice").is_none());
    }

    #[test]
    fn a_read_that_names_no_note_opens_the_closest_and_a_forget_does_not() {
        let (_base, verbs) = verbs("closest", true);
        seed(
            &verbs,
            "never-git-reset",
            "never git reset in the shared tree",
            "sync main another way",
        );
        seed(&verbs, "orb-motion", "orb motion taste", "least movement");
        seed(&verbs, "lane-main", "a lane merges main", "lanes");
        let mut query = Map::new();
        query.insert("name".to_owned(), Value::from("how do I sync main"));
        let reply = verbs.read(&query).unwrap();
        assert_eq!(reply["name"], "never-git-reset");
        assert_eq!(
            reply["warnings"][0],
            "[no note has that name or hook · opened never-git-reset, the closest of 2 by memory.search · memory.search(\"how do I sync main\", limit=2) for the ranking]"
        );
        assert!(verbs.forget(&query).is_err());
        query.insert("name".to_owned(), Value::from("zebra"));
        assert!(verbs.read(&query).is_err());
    }

    #[test]
    fn a_child_is_refused_every_verb() {
        let (_base, verbs) = verbs("child", false);
        let err = verbs.save(&save_payload("x")).unwrap_err();
        assert!(err.contains("only the root session"), "{err}");
        let mut name = Map::new();
        name.insert("name".to_owned(), Value::from("x"));
        assert!(verbs.read(&name).is_err());
        assert!(verbs.forget(&name).is_err());
    }

    #[test]
    fn read_counts_and_replies_without_a_path() {
        let (_base, verbs) = verbs("read", true);
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
