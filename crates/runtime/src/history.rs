use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Map, Value};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::wire::{JsonlV4Header, Mutation};

use crate::args::Args;
use crate::memory::rank::{Bm25, tokens};
use yi_session::SNIPPET_CHARS;

const PAGE: usize = 8;
const PAGE_MAX: usize = yi_session::GREP_PAGE_MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub session: String,
    pub entry: String,
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoSessions {
    pub files: Vec<PathBuf>,
    pub skipped: usize,
}

fn header_cwd(dir: &Path) -> Option<PathBuf> {
    let file = fs::File::open(jsonl_files(dir).into_iter().next()?).ok()?;
    let first = std::io::BufReader::new(file).lines().next()?.ok()?;
    let header: JsonlV4Header = serde_json::from_str(&first).ok()?;
    Some(PathBuf::from(header.cwd))
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Belongs {
    Mine,
    Other,
    Unknown,
}

fn belongs(dir: &Path, repo: &Path) -> Belongs {
    let Some(at) = header_cwd(dir) else {
        return Belongs::Other;
    };
    // Incident: a header's `/var/…` never matched its own `/private/var/…` repository.
    let at = at.canonicalize().unwrap_or(at);
    match repo_at(&at) {
        Some(other) if other == repo => Belongs::Mine,
        Some(_) => Belongs::Other,
        None if at.starts_with(repo) => Belongs::Mine,
        None if at.exists() => Belongs::Other,
        None => Belongs::Unknown,
    }
}

/// [`crate::lane::canonical_repo`] with its misses kept too: a gone checkout spawned a git
/// per session directory on every name lookup.
fn repo_at(at: &Path) -> Option<PathBuf> {
    static MISSED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let missed = |at: &Path| {
        MISSED
            .lock()
            .is_ok_and(|missed| missed.iter().any(|known| known == at))
    };
    if missed(at) {
        return None;
    }
    let repo = crate::lane::canonical_repo(at);
    if repo.is_none()
        && let Ok(mut missed) = MISSED.lock()
    {
        missed.push(at.to_path_buf());
    }
    repo
}

fn repo_of(cwd: &Path) -> PathBuf {
    crate::lane::canonical_repo(cwd)
        .unwrap_or_else(|| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()))
}

fn session_dirs(sessions_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(sessions_dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    dirs.retain(|dir| dir.is_dir());
    dirs.sort();
    dirs
}

pub fn repo_sessions(sessions_dir: &Path, cwd: &Path) -> RepoSessions {
    Corpus::default().sessions(sessions_dir, cwd)
}

/// Whether `file` is the session `id`: its name is `<created>_<id>.jsonl` exactly, so no other
/// spelling of the path can stand for the id a wall names.
fn carries(file: &Path, id: &str) -> bool {
    let stem = file
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".jsonl"));
    stem.and_then(|stem| stem.split_once('_'))
        .is_some_and(|(_, rest)| rest == id)
}

/// The session file carrying `id`: this repository's first, then any other repository's.
pub fn find_session(sessions_dir: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\']) {
        return None;
    }
    let mine = (repo_sessions(sessions_dir, cwd).files.into_iter()).find(|file| carries(file, id));
    let anywhere = || {
        (session_dirs(sessions_dir).iter())
            .flat_map(|dir| jsonl_files(dir))
            .find(|file| carries(file, id))
    };
    mine.or_else(anywhere)
}

/// A name as an address spells it: case aside, its spaces as dashes, since a URL holds none.
pub(crate) fn slug(name: &str) -> String {
    (name.trim().chars())
        .map(|character| {
            if character.is_whitespace() {
                '-'
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect()
}

fn same_name(name: &str, address: &str) -> bool {
    slug(name) == address.to_ascii_lowercase()
}

/// One session a name could mean: the address that reads it and the repository it ran in.
pub struct Candidate {
    pub id: String,
    pub cwd: String,
}

/// What a session name resolves to: the one session it names, or every candidate it fits.
pub enum Named {
    One(PathBuf),
    Many(Vec<Candidate>),
    Nothing,
}

/// The sessions named `name`, this repository's before any other's: a name taken here is never
/// shadowed by another repository's session of the same name.
pub fn find_named(sessions_dir: &Path, cwd: &Path, name: &str) -> Named {
    let repo = repo_of(cwd);
    let (mut mine, mut others) = (Vec::new(), Vec::new());
    for dir in session_dirs(sessions_dir) {
        let listed = yi_session::list_dir(&dir).unwrap_or_default();
        let named = (listed.into_iter()).filter(|session| {
            session
                .name
                .as_deref()
                .is_some_and(|it| same_name(it, name))
        });
        let files = jsonl_files(&dir);
        let found: Vec<(PathBuf, String)> = named
            .filter_map(|session| {
                Some((
                    files
                        .iter()
                        .find(|file| carries(file, &session.id))?
                        .clone(),
                    session.id,
                ))
            })
            .collect();
        match belongs(&dir, &repo) {
            Belongs::Mine => mine.extend(found),
            Belongs::Other | Belongs::Unknown => others.extend(found),
        }
    }
    let found = if mine.is_empty() { others } else { mine };
    match found.as_slice() {
        [] => Named::Nothing,
        [(file, _)] => Named::One(file.clone()),
        many => Named::Many(
            (many.iter())
                .map(|(file, id)| Candidate {
                    id: id.clone(),
                    cwd: file
                        .parent()
                        .and_then(header_cwd)
                        .map(|cwd| cwd.display().to_string())
                        .unwrap_or_default(),
                })
                .collect(),
        ),
    }
}

fn text_of(message: &AgentMessage) -> Option<(&'static str, String)> {
    let texts = |blocks: &[Content]| yi_types::message::join_text(blocks, "\n");
    match message {
        AgentMessage::User { content, .. } => Some((
            "user",
            match content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => texts(blocks),
            },
        )),
        AgentMessage::Assistant { content, .. } => Some(("assistant", texts(content))),
        _ => None,
    }
}

pub fn units_of(file: &Path) -> Vec<Unit> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let Some(header) = lines
        .next()
        .and_then(|line| serde_json::from_str::<JsonlV4Header>(line).ok())
    else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut units = Vec::new();
    for line in lines {
        let Ok(Mutation::Entry { entry, .. }) = serde_json::from_str::<Mutation>(line) else {
            continue;
        };
        let found = match &entry {
            Entry::Message { message, .. } => text_of(message),
            Entry::Compaction { summary, .. } | Entry::BranchSummary { summary, .. } => {
                Some(("summary", summary.clone()))
            }
            _ => None,
        };
        let Some((kind, body)) = found.filter(|(_, body)| !body.trim().is_empty()) else {
            continue;
        };
        if seen.insert(entry.id().to_owned()) {
            units.push(Unit {
                session: header.id.clone(),
                entry: entry.id().to_owned(),
                kind,
                text: body,
            });
        }
    }
    units
}

#[derive(Default)]
pub struct Corpus {
    dirs: Mutex<BTreeMap<PathBuf, Belongs>>,
    files: Mutex<BTreeMap<PathBuf, Cached>>,
}

/// Incident: every kernel kept its own copy of the repo's session text, and each query cloned
/// all of it; one corpus per process, shared by reference, holds it once.
static SHARED: LazyLock<Arc<Corpus>> = LazyLock::new(Arc::default);

type Cached = ((u64, u64), Arc<Vec<Unit>>);

/// Invariant: a torn-tail repair or fork replaces a session file by rename, which can keep its
/// length and mtime; the inode is what moves.
fn stamp_of(file: &Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(file).map_or((0, 0), |meta| (meta.len(), meta.ino()))
}

impl Corpus {
    pub fn sessions(&self, sessions_dir: &Path, cwd: &Path) -> RepoSessions {
        let repo = repo_of(cwd);
        let mut found = RepoSessions::default();
        for dir in session_dirs(sessions_dir) {
            let cached = self
                .dirs
                .lock()
                .ok()
                .and_then(|dirs| dirs.get(&dir).copied());
            let verdict = cached.unwrap_or_else(|| belongs(&dir, &repo));
            if let Ok(mut dirs) = self.dirs.lock() {
                dirs.insert(dir.clone(), verdict);
            }
            match verdict {
                Belongs::Mine => found.files.extend(jsonl_files(&dir)),
                Belongs::Other => {}
                Belongs::Unknown => found.skipped = found.skipped.saturating_add(1),
            }
        }
        found
    }

    pub fn units(&self, files: &[PathBuf]) -> Vec<Arc<Vec<Unit>>> {
        let Ok(mut cached) = self.files.lock() else {
            return files.iter().map(|file| Arc::new(units_of(file))).collect();
        };
        let mut all = Vec::new();
        for file in files {
            let stamp = stamp_of(file);
            let fresh = cached.get(file).is_some_and(|(seen, _)| *seen == stamp);
            if !fresh {
                cached.insert(file.clone(), (stamp, Arc::new(units_of(file))));
            }
            if let Some((_, units)) = cached.get(file) {
                all.push(Arc::clone(units));
            }
        }
        all
    }
}

pub fn search_reply(
    units: &[Arc<Vec<Unit>>],
    query: &str,
    offset: usize,
    limit: usize,
    skipped: usize,
) -> Map<String, Value> {
    let limit = limit.clamp(1, PAGE_MAX);
    let units: Vec<&Unit> = units.iter().flat_map(|file| file.iter()).collect();
    let ranked = Bm25::new(units.iter().map(|unit| unit.text.as_str())).rank(query);
    let words = tokens(query);
    let rows = (ranked.iter().skip(offset).take(limit))
        .filter_map(|(at, _)| units.get(*at).copied())
        .map(|unit| {
            let (text, cut) = yi_session::snippet(&unit.text, words.iter().map(String::as_str));
            let url = format!("history://{}/{}", unit.session, unit.entry);
            let snippet = (text.replace('\n', " "), cut);
            hit(Some(&unit.session), (&unit.entry, unit.kind), snippet, url)
        })
        .collect();
    let unsearched = (skipped > 0).then(|| {
        format!(
            "[{skipped} session directories were not searched: their checkout is gone, so their repository is unknown]"
        )
    });
    let call = ("compact.search", query);
    paged_reply(rows, ranked.len(), (offset, limit), call, unsearched, false)
}

fn hit(
    session: Option<&str>,
    (entry, kind): (&str, &str),
    (snippet, cut): (String, bool),
    url: String,
) -> Value {
    let head = session.map(|session| ("session", session));
    let rest = [
        ("entryId", entry),
        ("type", kind),
        ("snippet", &snippet),
        ("url", &url),
    ];
    let keyed = head.into_iter().chain(rest);
    let mut row: Map<String, Value> = keyed.map(|(key, text)| (key.into(), text.into())).collect();
    if cut {
        row.insert("cut".to_owned(), Value::Bool(true));
    }
    Value::Object(row)
}

fn paged_reply(
    rows: Vec<Value>,
    total: usize,
    (offset, limit): (usize, usize),
    (verb, query): (&str, &str),
    tail: Option<String>,
    walled: bool,
) -> Map<String, Value> {
    let (kept, cap) = (rows.len(), limit.clamp(1, PAGE_MAX));
    let after = offset.saturating_add(kept);
    let cut = rows.iter().filter(|row| row.get("cut").is_some()).count();
    let notices: Vec<String> = [
        (after < total).then(|| {
            let quoted = Value::from(query);
            format!("[{kept} of {total} hits · limit {cap} (at most {PAGE_MAX}) · {verb}({quoted}, limit={cap}, offset={after}) for the next]")
        }),
        (cut > 0).then(|| {
            let rest = match walled {
                true => "this reader's wall denies history://, so the rest of a cut hit is out of reach",
                false => "rlm.fetch(hit[\"url\"]) reads a cut hit whole",
            };
            format!("[{cut} of {kept} snippets cut to {SNIPPET_CHARS} chars (SNIPPET_CHARS) · {rest}]")
        }),
        tail,
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut reply = Map::new();
    reply.insert("hits".to_owned(), Value::from(rows));
    reply.insert("total".to_owned(), Value::from(total));
    if !notices.is_empty() {
        reply.insert("notice".to_owned(), Value::from(notices.join("\n")));
    }
    reply
}

fn page_of(payload: &Map<String, Value>) -> (usize, usize) {
    let number = |key: &str, default: usize| {
        (payload.u64_of(key)).map_or(default, |n| usize::try_from(n).unwrap_or(default))
    };
    (number("offset", 0), number("limit", PAGE))
}

fn grep_reply(
    hits: &[yi_session::HistoryHit],
    total: usize,
    pattern: &str,
    offset: usize,
    limit: usize,
    walled: bool,
) -> Map<String, Value> {
    let rows = (hits.iter())
        .map(|found| {
            let url = format!("history://{}/{}", crate::fetch::SELF, found.entry_id);
            let snippet = (found.snippet.clone(), found.cut);
            hit(None, (&found.entry_id, &found.entry_type), snippet, url)
        })
        .collect();
    let call = ("compact.recall", pattern);
    paged_reply(rows, total, (offset, limit), call, None, walled)
}

pub fn register_grep(
    registry: &mut crate::kernel::HostRegistry,
    handle: crate::goal::StoreHandle,
    wall: &crate::wall::Wall,
) {
    let walled = wall.denies_history();
    registry.register("history.grep", move |payload| {
        let handle = Arc::clone(&handle);
        Box::pin(async move {
            let pattern = payload
                .str_of("pattern")
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .ok_or_else(|| "history.grep requires a \"pattern\" argument".to_owned())?
                .to_owned();
            let (offset, limit) = page_of(&payload);
            let page = tokio::task::spawn_blocking(move || {
                let shared = handle()
                    .ok_or_else(|| "history.grep: missing an attached session store".to_owned())?;
                let store = yi_session::lock_session(&shared);
                Ok::<_, String>((store.grep_page(&pattern, offset, limit), pattern))
            })
            .await
            .map_err(|error| format!("history.grep task failed: {error}"))??;
            let ((hits, total), pattern) = page;
            Ok(grep_reply(&hits, total, &pattern, offset, limit, walled))
        })
    });
}

pub fn register(registry: &mut crate::kernel::HostRegistry, sessions_dir: PathBuf, cwd: PathBuf) {
    let corpus = Arc::clone(&SHARED);
    registry.register("history.search", move |payload| {
        let (corpus, sessions_dir, cwd) = (Arc::clone(&corpus), sessions_dir.clone(), cwd.clone());
        Box::pin(async move {
            let query = payload
                .str_of("query")
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .ok_or_else(|| "history.search requires a \"query\" argument".to_owned())?
                .to_owned();
            let (offset, limit) = page_of(&payload);
            tokio::task::spawn_blocking(move || {
                let found = corpus.sessions(&sessions_dir, &cwd);
                let units = corpus.units(&found.files);
                search_reply(&units, &query, offset, limit, found.skipped)
            })
            .await
            .map_err(|error| format!("history.search task failed: {error}"))
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    fn git(dir: &Path, args: &[&str]) {
        let out = yi_tools::command("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn session(sessions: &Path, cwd: &Path, id: &str, messages: &[Value]) {
        let mut repo = yi_session::JsonlRepo::new(sessions.to_path_buf(), cwd.to_string_lossy());
        let options = yi_session::CreateOptions {
            id: Some(id.to_owned()),
            parent_session_id: None,
            metadata: None,
        };
        let shared = yi_session::SessionRepo::create(&mut repo, options).unwrap();
        let mut store = yi_session::lock_session(&shared);
        for message in messages {
            let message: AgentMessage = serde_json::from_value(message.clone()).unwrap();
            store.append_message("main", message).unwrap();
        }
    }

    fn user(text: &str) -> Value {
        serde_json::json!({"role": "user", "content": [{"type": "text", "text": text}], "timestamp": 1})
    }

    fn tool_call(text: &str, argument: &str) -> Value {
        serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": text},
                {"type": "toolCall", "id": "call-1", "name": "ipython", "arguments": {"code": argument}}
            ],
            "api": "faux", "provider": "faux", "model": "faux-1",
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                      "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
            "stopReason": "toolUse", "timestamp": 2
        })
    }

    #[test]
    fn a_search_reaches_every_lane_of_the_repository_and_skips_tool_arguments() {
        let root = Scratch::new("yi-history-lanes").unwrap();
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        let base = ["-c", "user.email=m@t", "-c", "user.name=m", "commit", "-q"];
        git(
            &repo,
            &[&base[..], &["--allow-empty", "-m", "base"]].concat(),
        );
        git(&repo, &["worktree", "add", "-q", "../lane", "HEAD"]);
        let other = root.join("other");
        fs::create_dir_all(&other).unwrap();
        git(&other, &["init", "-q", "-b", "main"]);
        let sessions = root.join("sessions");
        let lane = [
            user("never scratch under tmp on the buildhost host"),
            tool_call("saving that", "await memory.save('zebrafish tmp body')"),
        ];
        session(
            &sessions,
            &repo,
            "s-main",
            &[user("the ö probe HOME sat under tmp")],
        );
        session(&sessions, &root.join("lane"), "s-lane", &lane);
        session(
            &sessions,
            &other,
            "s-other",
            &[user("tmp in another repository")],
        );
        session(
            &sessions,
            &root.join("gone"),
            "s-gone",
            &[user("tmp from a deleted checkout")],
        );

        let corpus = Corpus::default();
        let found = corpus.sessions(&sessions, &repo);
        assert_eq!((found.files.len(), found.skipped), (2, 1), "{found:?}");
        let units = corpus.units(&found.files);
        let reply = search_reply(&units, "scratch tmp", 0, 8, found.skipped);
        assert_eq!(reply["hits"][0]["session"], "s-lane");
        let entry = reply["hits"][0]["entryId"].as_str().unwrap().to_owned();
        assert_eq!(reply["hits"][0]["url"], format!("history://s-lane/{entry}"));
        assert_eq!(reply["total"], 2, "{reply:?}");
        let row = reply["hits"][0].as_object().into_iter();
        let keys: Vec<&str> = row.flat_map(|row| row.keys().map(String::as_str)).collect();
        assert_eq!(keys, ["session", "entryId", "type", "snippet", "url"]);
        assert_eq!(
            reply["notice"],
            "[1 session directories were not searched: their checkout is gone, so their repository is unknown]"
        );
        assert_eq!(search_reply(&units, "zebrafish", 0, 8, 0)["total"], 0);
        let page = search_reply(&units, "tmp", 0, 1, 0);
        assert_eq!(
            page["notice"],
            "[1 of 2 hits · limit 1 (at most 32) · compact.search(\"tmp\", limit=1, offset=1) for the next]"
        );
        let file = find_session(&sessions, &repo, "s-lane").unwrap();
        let loaded = yi_session::load_session(&file).unwrap();
        assert!(loaded.entry(&entry).is_some());
        let other = find_session(&sessions, &repo, "s-other");
        assert!(
            other.is_some(),
            "a read reaches another repository's session by id"
        );
    }

    #[test]
    fn a_session_replaced_at_the_same_length_and_time_is_reread()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = Scratch::new("yi-history-replace")?;
        let (sessions, cwd) = (root.join("sessions"), root.join("cwd"));
        fs::create_dir_all(&cwd)?;
        session(&sessions, &cwd, "s-one", &[user("alpha bravo")]);
        let file = find_session(&sessions, &cwd, "s-one").ok_or("no session file")?;
        let corpus = Corpus::default();
        assert_eq!(
            search_reply(&corpus.units(std::slice::from_ref(&file)), "alpha", 0, 8, 0)["total"],
            1
        );
        let when = fs::metadata(&file)?.modified()?;
        let swapped = fs::read_to_string(&file)?.replace("alpha", "gamma");
        let next = file.with_extension("tmp");
        fs::write(&next, swapped)?;
        fs::rename(&next, &file)?;
        fs::File::options()
            .write(true)
            .open(&file)?
            .set_modified(when)?;
        let units = corpus.units(&[file]);
        assert_eq!(search_reply(&units, "alpha", 0, 8, 0)["total"], 0);
        assert_eq!(search_reply(&units, "gamma", 0, 8, 0)["total"], 1);
        Ok(())
    }

    #[test]
    fn a_cut_grep_page_names_the_call_for_the_next_page() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut store = yi_session::SessionStore::in_memory(yi_types::wire::SessionMetadata {
            id: "s".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        });
        for turn in 0..10 {
            store.append_compaction(
                "main",
                format!("the ö needle, turn {turn}"),
                Vec::new(),
                1,
                None,
            )?;
        }
        let (hits, total) = store.grep_page("Needle", 0, 8);
        let reply = grep_reply(&hits, total, "Needle", 0, 8, false);
        assert_eq!(reply["total"], 10);
        assert_eq!(
            reply["notice"],
            "[8 of 10 hits · limit 8 (at most 32) · compact.recall(\"Needle\", limit=8, offset=8) for the next]"
        );
        let (rest, total) = store.grep_page("Needle", 8, 8);
        let last = grep_reply(&rest, total, "Needle", 8, 8, false);
        assert_eq!(last["hits"].as_array().map(Vec::len), Some(2));
        assert!(last.get("notice").is_none(), "{last:?}");
        let (clamped, total) = store.grep_page("needle", 0, 500);
        let wide = grep_reply(&clamped, total, "needle", 0, 500, false);
        assert_eq!(wide["hits"].as_array().map(Vec::len), Some(10));
        assert!(wide.get("notice").is_none());
        Ok(())
    }

    #[test]
    fn a_page_at_the_limit_has_no_notice_and_one_past_it_names_the_next_offset()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = Scratch::new("yi-history-limit")?;
        let cwd = root.join("cwd");
        fs::create_dir_all(&cwd)?;
        for (count, more) in [(PAGE_MAX, false), (PAGE_MAX + 1, true)] {
            let said: Vec<Value> = (0..count)
                .map(|n| user(&format!("note {n} on the limit")))
                .collect();
            let sessions = root.join(format!("sessions-{count}"));
            session(&sessions, &cwd, "s-one", &said);
            let file = find_session(&sessions, &cwd, "s-one").ok_or("no session file")?;
            let units = Corpus::default().units(std::slice::from_ref(&file));
            let found = search_reply(&units, "limit", 0, PAGE_MAX, 0);
            let (hits, total) = yi_session::load_session(&file)?.grep_page("limit", 0, PAGE_MAX);
            for (reply, call) in [
                (found, "search"),
                (
                    grep_reply(&hits, total, "limit", 0, PAGE_MAX, false),
                    "recall",
                ),
            ] {
                let next = format!("compact.{call}(\"limit\", limit=32, offset=32)");
                let want = more.then(|| {
                    Value::from(format!(
                        "[32 of 33 hits · limit 32 (at most 32) · {next} for the next]"
                    ))
                });
                assert_eq!(reply["total"], count, "{call}");
                assert_eq!(reply.get("notice"), want.as_ref(), "{call} at {count}");
            }
        }
        Ok(())
    }
}
