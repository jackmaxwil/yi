//! `session/list` from `.index.json`: each file's header and name state under its length and
//! mtime. Only an append grows a session, so a list reads new files whole and grown ones' tails.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::{Map, Value, json};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::wire::{Fact, HeaderKind, JsonlV4Header, Mutation};

use crate::error::SessionError;
use crate::id::session_title;
use crate::jsonl::metadata_from_header;
use crate::query::SessionMetadata;

pub(crate) const INDEX: &str = ".index.json";

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NameScan {
    fact: Option<String>,
    prompt: Option<String>,
}

impl NameScan {
    pub(crate) fn feed(&mut self, line: &str) {
        if line.contains(r#""fact":"name""#) {
            if let Ok(Mutation::Fact {
                fact: Fact::Name { name },
                ..
            }) = serde_json::from_str::<Mutation>(line)
            {
                self.fact = name;
            }
        } else if self.prompt.is_none()
            && line.contains(r#""role":"user""#)
            && let Ok(Mutation::Entry {
                entry:
                    Entry::Message {
                        message: AgentMessage::User { content, .. },
                        ..
                    },
                ..
            }) = serde_json::from_str::<Mutation>(line)
        {
            let text = match content {
                UserContent::Text(text) => text,
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            self.prompt = session_title(&text);
        }
    }

    pub(crate) fn name(&self) -> Option<String> {
        self.fact.clone().or_else(|| self.prompt.clone())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Listed {
    len: u64,
    mtime: u64,
    scanned: u64,
    session: Option<SessionMetadata>,
    names: NameScan,
}

fn mtime_ns(stat: &fs::Metadata) -> u64 {
    stat.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

fn header_metadata(line: &str) -> Option<SessionMetadata> {
    serde_json::from_str::<JsonlV4Header>(line)
        .ok()
        .filter(|header| header.kind == HeaderKind::Header && header.version == 4)
        .map(|header| metadata_from_header(&header))
}

fn rescan(path: &Path, old: Option<Listed>, len: u64, mtime: u64) -> Option<Listed> {
    let mut listed = old
        .filter(|old| old.session.is_some() && len >= old.len)
        .unwrap_or_default();
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(listed.scanned)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split('\n');
    if listed.scanned == 0 {
        listed.session = lines.next().and_then(header_metadata);
    }
    if listed.session.is_some() {
        lines.for_each(|line| listed.names.feed(line));
    }
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    listed.scanned = listed
        .scanned
        .saturating_add(u64::try_from(complete).unwrap_or(u64::MAX));
    listed.len = len;
    listed.mtime = mtime;
    Some(listed)
}

fn to_json(listed: &Listed) -> Value {
    let session = listed.session.as_ref();
    json!({
        "len": listed.len,
        "mtime": listed.mtime,
        "scanned": listed.scanned,
        "id": session.map(|session| session.id.clone()),
        "createdAt": session.map(|session| session.created_at),
        "parent": session.and_then(|session| session.parent_session_id.clone()),
        "fact": listed.names.fact,
        "prompt": listed.names.prompt,
    })
}

fn from_json(value: &Value) -> Option<Listed> {
    let number = |key: &str| value.get(key).and_then(Value::as_u64);
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let session = text("id").map(|id| SessionMetadata {
        id,
        created_at: number("createdAt").unwrap_or(0),
        parent_session_id: text("parent"),
        name: None,
    });
    Some(Listed {
        len: number("len")?,
        mtime: number("mtime")?,
        scanned: number("scanned")?,
        session,
        names: NameScan {
            fact: text("fact"),
            prompt: text("prompt"),
        },
    })
}

fn load_index(dir: &Path) -> HashMap<String, Listed> {
    let text = fs::read_to_string(dir.join(INDEX)).unwrap_or_default();
    let files = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(mut index)) => match index.remove("files") {
            Some(Value::Object(files)) => files,
            _ => Map::new(),
        },
        _ => Map::new(),
    };
    files
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), from_json(value)?)))
        .collect()
}

fn save_index(dir: &Path, files: &HashMap<String, Listed>) {
    let files: Map<String, Value> = files
        .iter()
        .map(|(name, listed)| (name.clone(), to_json(listed)))
        .collect();
    let temp = dir.join(format!("{INDEX}.{}.tmp", std::process::id()));
    let body = json!({"version": 1, "files": files}).to_string();
    if fs::write(&temp, body).is_ok() && fs::rename(&temp, dir.join(INDEX)).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

pub(crate) fn list(dir: &Path) -> Result<Vec<SessionMetadata>, SessionError> {
    let storage = |error: std::io::Error| {
        SessionError::Storage(format!(
            "Failed to list sessions directory {}: {error}",
            dir.display()
        ))
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(storage(error)),
    };
    let mut index = load_index(dir);
    let mut current = HashMap::new();
    let mut changed = false;
    for entry in entries {
        let entry = entry.map_err(storage)?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let Ok(stat) = entry.metadata() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let (len, mtime) = (stat.len(), mtime_ns(&stat));
        let listed = match index.remove(&name) {
            Some(known) if known.len == len && known.mtime == mtime => known,
            known => {
                changed = true;
                let Some(listed) = rescan(&path, known, len, mtime) else {
                    continue;
                };
                listed
            }
        };
        current.insert(name, listed);
    }
    if changed || !index.is_empty() {
        save_index(dir, &current);
    }
    let mut listed: Vec<SessionMetadata> = current
        .into_values()
        .filter_map(|listed| {
            let name = listed.names.name();
            listed
                .session
                .map(|session| SessionMetadata { name, ..session })
        })
        .collect();
    listed.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(listed)
}
