use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::wire::{JsonlV4Header, Mutation};

use crate::memory::rank::{Bm25, tokens};

const PAGE: usize = 8;
const PAGE_MAX: usize = 32;
const SNIPPET_CHARS: usize = 160;

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
    match crate::lane::canonical_repo(&at) {
        Some(other) if other == repo => Belongs::Mine,
        Some(_) => Belongs::Other,
        None if at.starts_with(repo) => Belongs::Mine,
        None if at.exists() => Belongs::Other,
        None => Belongs::Unknown,
    }
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

pub fn find_session(sessions_dir: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("_{id}.jsonl");
    repo_sessions(sessions_dir, cwd)
        .files
        .into_iter()
        .find(|file| file.to_string_lossy().ends_with(&suffix))
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

fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let first = tokens(query)
        .iter()
        .filter_map(|token| lower.find(token.as_str()))
        .min()
        .unwrap_or(0);
    let at = lower
        .char_indices()
        .take_while(|(byte, _)| *byte < first)
        .count();
    let start = at.saturating_sub(SNIPPET_CHARS / 2);
    text.chars()
        .skip(start)
        .take(SNIPPET_CHARS)
        .collect::<String>()
        .replace('\n', " ")
}

#[derive(Default)]
pub struct Corpus {
    dirs: Mutex<BTreeMap<PathBuf, Belongs>>,
    files: Mutex<BTreeMap<PathBuf, (u64, Vec<Unit>)>>,
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

    pub fn units(&self, files: &[PathBuf]) -> Vec<Unit> {
        let Ok(mut cached) = self.files.lock() else {
            return files.iter().flat_map(|file| units_of(file)).collect();
        };
        let mut all = Vec::new();
        for file in files {
            let len = fs::metadata(file).map_or(0, |meta| meta.len());
            let fresh = cached.get(file).is_some_and(|(seen, _)| *seen == len);
            if !fresh {
                cached.insert(file.clone(), (len, units_of(file)));
            }
            if let Some((_, units)) = cached.get(file) {
                all.extend(units.iter().cloned());
            }
        }
        all
    }
}

pub fn search_reply(
    units: &[Unit],
    query: &str,
    offset: usize,
    limit: usize,
    skipped: usize,
) -> Map<String, Value> {
    let limit = limit.clamp(1, PAGE_MAX);
    let ranked = Bm25::new(units.iter().map(|unit| unit.text.as_str())).rank(query);
    let total = ranked.len();
    let hits: Vec<Value> = ranked
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|(at, _)| units.get(*at))
        .map(|unit| {
            let mut item = Map::new();
            item.insert("session".to_owned(), Value::from(unit.session.as_str()));
            item.insert("entryId".to_owned(), Value::from(unit.entry.as_str()));
            item.insert("type".to_owned(), Value::from(unit.kind));
            item.insert(
                "snippet".to_owned(),
                Value::from(snippet(&unit.text, query)),
            );
            item.insert(
                "url".to_owned(),
                Value::from(format!("history://{}/{}", unit.session, unit.entry)),
            );
            Value::Object(item)
        })
        .collect();
    let mut notices = Vec::new();
    let next = offset.saturating_add(hits.len());
    if next < total {
        notices.push(format!(
            "[{} of {total} hits · limit {limit} (at most {PAGE_MAX}) · compact.search({}, limit={limit}, offset={next}) for the next]",
            hits.len(),
            Value::from(query)
        ));
    }
    if skipped > 0 {
        notices.push(format!(
            "[{skipped} session directories were not searched: their checkout is gone, so their repository is unknown]"
        ));
    }
    let mut reply = Map::new();
    reply.insert("hits".to_owned(), Value::from(hits));
    reply.insert("total".to_owned(), Value::from(total));
    if !notices.is_empty() {
        reply.insert("notice".to_owned(), Value::from(notices.join("\n")));
    }
    reply
}

pub fn register(registry: &mut crate::kernel::HostRegistry, sessions_dir: PathBuf, cwd: PathBuf) {
    let corpus = Arc::new(Corpus::default());
    registry.register("history.search", move |payload| {
        let (corpus, sessions_dir, cwd) = (Arc::clone(&corpus), sessions_dir.clone(), cwd.clone());
        Box::pin(async move {
            let query = payload
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .ok_or_else(|| "history.search requires a \"query\" argument".to_owned())?
                .to_owned();
            let number = |key: &str, default: usize| {
                payload
                    .get(key)
                    .and_then(Value::as_u64)
                    .map_or(default, |n| usize::try_from(n).unwrap_or(default))
            };
            let (offset, limit) = (number("offset", 0), number("limit", PAGE));
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
        assert!(find_session(&sessions, &repo, "s-other").is_none());
    }
}
