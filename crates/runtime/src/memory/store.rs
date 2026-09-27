use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use yi_types::plan::canonical::Digest;

use super::doc::{Memory, MemoryName, Note, read_note};
use super::journal::{self, Journal};

const INDEX: &str = "MEMORY.md";
const USAGE: &str = "usage.json";
const LOAD_LINES: usize = 200;
const LOAD_BYTES: usize = 25_000;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("{file} exists but does not read; the user must fix or remove it, nothing was changed")]
    Unreadable { file: String },
    #[error("{file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
    #[error("ops.jsonl: a record did not serialize: {detail}")]
    Journal { detail: String },
}

fn io(file: &str) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        file: file.to_owned(),
        source,
    }
}

pub fn repo_dir(home: &Path, cwd: &Path) -> PathBuf {
    let key = crate::lane::canonical_repo(cwd)
        .unwrap_or_else(|| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()));
    home.join(".yi/projects")
        .join(yi_session::session_directory_name(&key.to_string_lossy()))
        .join("memory")
}

pub fn global_dir(home: &Path) -> PathBuf {
    home.join(".yi/memory")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexLine {
    Note { name: MemoryName, text: String },
    Other(String),
}

fn link_target(line: &str) -> Option<MemoryName> {
    let open = line.find("](")?;
    let rest = line.get(open.saturating_add(2)..)?;
    let target = rest.get(..rest.find(')')?)?;
    MemoryName::from_stem(target.strip_suffix(".md")?)
}

fn parse_index(text: &str) -> Vec<IndexLine> {
    text.lines()
        .map(|line| match link_target(line) {
            Some(name) => IndexLine::Note {
                name,
                text: line.to_owned(),
            },
            None => IndexLine::Other(line.to_owned()),
        })
        .collect()
}

fn render_index(lines: &[IndexLine]) -> String {
    let joined = lines
        .iter()
        .map(|line| match line {
            IndexLine::Note { text, .. } | IndexLine::Other(text) => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = joined.trim_end().to_owned();
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoteUsage {
    pub saves: u64,
    pub reads: u64,
    pub last: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    pub sessions: u64,
    pub notes: BTreeMap<String, NoteUsage>,
}

fn number(map: &Map<String, Value>, key: &str) -> u64 {
    map.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn parse_usage(text: &str) -> Option<Usage> {
    let Value::Object(root) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    let notes = root
        .get("notes")
        .and_then(Value::as_object)
        .map(|notes| {
            notes
                .iter()
                .filter_map(|(name, entry)| {
                    let entry = entry.as_object()?;
                    Some((
                        name.clone(),
                        NoteUsage {
                            saves: number(entry, "saves"),
                            reads: number(entry, "reads"),
                            last: number(entry, "last"),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Usage {
        sessions: number(&root, "sessions"),
        notes,
    })
}

fn render_usage(usage: &Usage) -> String {
    let notes: Map<String, Value> = usage
        .notes
        .iter()
        .map(|(name, entry)| {
            let mut fields = Map::new();
            fields.insert("saves".to_owned(), Value::from(entry.saves));
            fields.insert("reads".to_owned(), Value::from(entry.reads));
            fields.insert("last".to_owned(), Value::from(entry.last));
            (name.clone(), Value::Object(fields))
        })
        .collect();
    let mut root = Map::new();
    root.insert("sessions".to_owned(), Value::from(usage.sessions));
    root.insert("notes".to_owned(), Value::Object(notes));
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    text.push('\n');
    text
}

pub fn now() -> u64 {
    #[expect(
        clippy::disallowed_methods,
        reason = "usage.json keeps when a note was last used; a clock read is the job"
    )]
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Imported {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    pub updated: bool,
    pub hash: Digest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rebuilt {
    pub live: usize,
    pub restored: usize,
    pub forgotten: usize,
    pub differ: Vec<String>,
    pub missing: Vec<String>,
    pub broken: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
    session: Option<String>,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, session: None }
    }

    pub fn with_session(mut self, session: Option<String>) -> Self {
        self.session = session;
        self
    }

    fn journal(&self) -> Journal<'_> {
        Journal::new(&self.dir)
    }

    #[cfg(test)]
    pub fn journal_sessions(&self) -> Vec<Option<String>> {
        let replay = self.journal().replay().unwrap_or_default();
        replay
            .records
            .into_iter()
            .map(|record| record.session)
            .collect()
    }

    fn append(&self, op: &str, name: &MemoryName, hash: Digest) -> Result<(), StoreError> {
        self.journal()
            .append(op, name.as_str(), hash, self.session.as_deref(), Map::new())
            .map(|_record| ())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> Result<fs::File, StoreError> {
        fs::create_dir_all(&self.dir).map_err(io("memory directory"))?;
        let file = fs::File::create(self.dir.join(".lock")).map_err(io(".lock"))?;
        file.lock().map_err(io(".lock"))?;
        Ok(file)
    }

    fn replace(&self, file: &str, text: &str) -> Result<(), StoreError> {
        let target = self.dir.join(file);
        let staging = self.dir.join(format!(".{file}.tmp"));
        fs::write(&staging, text)
            .and_then(|()| fs::rename(&staging, &target))
            .map_err(io(file))
    }

    fn read_text(&self, file: &str) -> Result<Option<String>, StoreError> {
        match fs::read(self.dir.join(file)) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| StoreError::Unreadable {
                    file: file.to_owned(),
                }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io(file)(error)),
        }
    }

    fn index(&self) -> Result<Vec<IndexLine>, StoreError> {
        Ok(self
            .read_text(INDEX)?
            .map(|text| parse_index(&text))
            .unwrap_or_default())
    }

    fn usage_strict(&self) -> Result<Usage, StoreError> {
        match self.read_text(USAGE)? {
            None => Ok(Usage::default()),
            Some(text) => parse_usage(&text).ok_or_else(|| StoreError::Unreadable {
                file: USAGE.to_owned(),
            }),
        }
    }

    pub fn usage(&self) -> Usage {
        self.usage_strict().unwrap_or_default()
    }

    fn note_at(&self, name: MemoryName) -> Option<Note> {
        let bytes = fs::read(self.dir.join(name.file())).ok()?;
        Some(read_note(name, &String::from_utf8_lossy(&bytes)))
    }

    pub fn notes(&self) -> Vec<Note> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut names: Vec<MemoryName> = entries
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name();
                let file = file.to_str()?;
                MemoryName::from_stem(file.strip_suffix(".md")?)
            })
            .collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| self.note_at(name))
            .collect()
    }

    pub fn resolve(&self, query: &str) -> Option<Note> {
        let query = query.trim();
        let by_name = MemoryName::from_stem(query)
            .into_iter()
            .chain(MemoryName::slug(query).ok())
            .find_map(|name| self.note_at(name));
        if by_name.is_some() {
            return by_name;
        }
        let wanted = query.to_lowercase();
        let hooked = |text: &str| text.trim().to_lowercase() == wanted;
        let from_index = self.index().ok().and_then(|lines| {
            lines.into_iter().find_map(|line| match line {
                IndexLine::Note { name, text } => {
                    let label = text
                        .split_once('[')
                        .and_then(|(_, rest)| rest.split_once(']'));
                    let hook = text.split_once(" — ").map(|(_, hook)| hook);
                    let hit =
                        label.is_some_and(|(label, _)| hooked(label)) || hook.is_some_and(hooked);
                    hit.then_some(name)
                }
                IndexLine::Other(_) => None,
            })
        });
        from_index
            .and_then(|name| self.note_at(name))
            .or_else(|| self.notes().into_iter().find(|note| hooked(&note.hook)))
    }

    pub fn reconcile(&self) -> Result<Vec<IndexLine>, StoreError> {
        if !self.dir.is_dir() {
            return Ok(Vec::new());
        }
        let _lock = self.lock()?;
        self.sync_journal()?;
        self.reconcile_index()
    }

    fn sync_journal(&self) -> Result<(), StoreError> {
        let journal = self.journal();
        let replay = journal.replay()?;
        let heads = replay.heads();
        let notes = self.notes();
        for note in &notes {
            let Some(text) = self.read_text(&note.name.file())? else {
                continue;
            };
            let hash = Digest::of(text.as_bytes());
            let op = match heads.get(note.name.as_str()) {
                Some(head) if head.op == journal::FORGET => journal::ADOPT,
                Some(head) if head.hash == hash => continue,
                Some(_) => journal::EDIT,
                None => journal::ADOPT,
            };
            journal.put_object(&text)?;
            self.append(op, &note.name, hash)?;
        }
        for (name, head) in &heads {
            if head.op == journal::FORGET || notes.iter().any(|note| note.name.as_str() == *name) {
                continue;
            }
            journal.drop_objects(&replay, name)?;
            let mut by = Map::new();
            by.insert("by".to_owned(), Value::from("hand"));
            journal.append(
                journal::FORGET,
                name,
                head.hash,
                self.session.as_deref(),
                by,
            )?;
        }
        Ok(())
    }

    fn reconcile_index(&self) -> Result<Vec<IndexLine>, StoreError> {
        let lines = self.index()?;
        let notes = self.notes();
        let mut seen: Vec<MemoryName> = Vec::new();
        let mut kept: Vec<IndexLine> = Vec::new();
        for line in &lines {
            match line {
                IndexLine::Note { name, .. } => {
                    if seen.contains(name) || !notes.iter().any(|note| &note.name == name) {
                        continue;
                    }
                    seen.push(name.clone());
                    kept.push(line.clone());
                }
                IndexLine::Other(_) => kept.push(line.clone()),
            }
        }
        for note in &notes {
            if !seen.contains(&note.name) {
                kept.push(IndexLine::Note {
                    name: note.name.clone(),
                    text: format!("- [{}]({}) — {}", note.name, note.name.file(), note.hook),
                });
            }
        }
        if kept != lines {
            self.replace(INDEX, &render_index(&kept))?;
        }
        Ok(kept)
    }

    pub fn save(&self, mut memory: Memory) -> Result<Saved, StoreError> {
        let _lock = self.lock()?;
        let mut usage = self.usage_strict()?;
        let mut lines = self.index()?;
        let old = self.note_at(memory.name.clone());
        if let Some(old) = &old {
            memory.carry(old);
        }
        let text = memory.render();
        let hash = self.journal().put_object(&text)?;
        self.replace(&memory.name.file(), &text)?;
        let line = IndexLine::Note {
            name: memory.name.clone(),
            text: memory.index_line(),
        };
        match lines.iter_mut().find(
            |existing| matches!(existing, IndexLine::Note { name, .. } if *name == memory.name),
        ) {
            Some(existing) => *existing = line,
            None => lines.push(line),
        }
        self.replace(INDEX, &render_index(&lines))?;
        let entry = usage.notes.entry(memory.name.to_string()).or_default();
        entry.saves = entry.saves.saturating_add(1);
        entry.last = now();
        self.replace(USAGE, &render_usage(&usage))?;
        self.append(journal::SAVE, &memory.name, hash)?;
        Ok(Saved {
            updated: old.is_some(),
            hash,
        })
    }

    pub fn forget(&self, name: &MemoryName) -> Result<Digest, StoreError> {
        let _lock = self.lock()?;
        let mut usage = self.usage_strict()?;
        let mut lines = self.index()?;
        let journal = self.journal();
        let replay = journal.replay()?;
        let hash = match self.read_text(&name.file())? {
            Some(text) => Digest::of(text.as_bytes()),
            None => replay
                .heads()
                .get(name.as_str())
                .map_or_else(|| Digest::of(b""), |head| head.hash),
        };
        journal.drop_objects(&replay, name.as_str())?;
        match fs::remove_file(self.dir.join(name.file())) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io(&name.file())(error)),
        }
        lines.retain(|line| !matches!(line, IndexLine::Note { name: own, .. } if own == name));
        self.replace(INDEX, &render_index(&lines))?;
        if usage.notes.remove(name.as_str()).is_some() {
            self.replace(USAGE, &render_usage(&usage))?;
        }
        self.append(journal::FORGET, name, hash)?;
        Ok(hash)
    }

    pub fn mark_read(&self, name: &MemoryName) -> Result<Digest, StoreError> {
        let _lock = self.lock()?;
        let hash = Digest::of(self.read_text(&name.file())?.unwrap_or_default().as_bytes());
        let mut usage = self.usage_strict()?;
        let entry = usage.notes.entry(name.to_string()).or_default();
        entry.reads = entry.reads.saturating_add(1);
        entry.last = now();
        self.replace(USAGE, &render_usage(&usage))?;
        self.append(journal::READ, name, hash)?;
        Ok(hash)
    }

    pub fn rebuild(&self) -> Result<Rebuilt, StoreError> {
        let mut report = Rebuilt::default();
        if !self.dir.is_dir() {
            return Ok(report);
        }
        let _lock = self.lock()?;
        let journal = self.journal();
        let replay = journal.replay()?;
        report.broken = replay.broken;
        let mut usage = Usage {
            sessions: self.usage().sessions,
            ..Usage::default()
        };
        for record in &replay.records {
            let entry = usage.notes.entry(record.name.clone()).or_default();
            match record.op.as_str() {
                journal::SAVE => entry.saves = entry.saves.saturating_add(1),
                journal::READ => entry.reads = entry.reads.saturating_add(1),
                _ => continue,
            }
            entry.last = entry.last.max(record.at);
        }
        for (name, head) in replay.heads() {
            if head.op == journal::FORGET {
                usage.notes.remove(name);
                report.forgotten = report.forgotten.saturating_add(1);
                continue;
            }
            report.live = report.live.saturating_add(1);
            let Some(file) = MemoryName::from_stem(name).map(|name| name.file()) else {
                continue;
            };
            match (self.read_text(&file)?, journal.object(&head.hash)) {
                (Some(text), _) if Digest::of(text.as_bytes()) == head.hash => {}
                (Some(_), _) => report.differ.push(name.to_owned()),
                (None, Some(text)) => {
                    self.replace(&file, &text)?;
                    report.restored = report.restored.saturating_add(1);
                }
                (None, None) => report.missing.push(name.to_owned()),
            }
        }
        usage
            .notes
            .retain(|name, _| MemoryName::from_stem(name).is_some());
        self.replace(USAGE, &render_usage(&usage))?;
        self.reconcile_index()?;
        Ok(report)
    }

    pub fn record(&self, change: impl FnOnce(&mut Usage)) -> Result<(), StoreError> {
        if !self.dir.is_dir() {
            return Ok(());
        }
        let _lock = self.lock()?;
        let mut usage = self.usage_strict()?;
        change(&mut usage);
        self.replace(USAGE, &render_usage(&usage))
    }

    pub fn import(&self, from: &Path) -> Result<Imported, StoreError> {
        let source = Self::new(from.to_path_buf());
        let mut report = Imported::default();
        {
            let _lock = self.lock()?;
            for note in source.notes() {
                let file = note.name.file();
                let bytes = fs::read(from.join(&file)).map_err(io(&file))?;
                match fs::read(self.dir.join(&file)) {
                    Ok(existing) if existing == bytes => {
                        report.skipped = report.skipped.saturating_add(1);
                        continue;
                    }
                    Ok(_) => report.updated = report.updated.saturating_add(1),
                    Err(_) => report.imported = report.imported.saturating_add(1),
                }
                let text = String::from_utf8_lossy(&bytes);
                self.replace(&file, &text)?;
            }
            let mut lines = self.index()?;
            for line in source.index()? {
                if let IndexLine::Note { name, .. } = &line {
                    let indexed = lines.iter().any(
                        |own| matches!(own, IndexLine::Note { name: have, .. } if have == name),
                    );
                    if !indexed && self.dir.join(name.file()).is_file() {
                        lines.push(line);
                    }
                }
            }
            self.replace(INDEX, &render_index(&lines))?;
        }
        self.reconcile()?;
        Ok(report)
    }
}

pub fn loaded<'a>(lines: &'a [IndexLine], usage: &Usage) -> (Vec<&'a IndexLine>, usize) {
    let notes: Vec<(usize, &IndexLine)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| matches!(line, IndexLine::Note { .. }))
        .collect();
    let mut ranked = notes.clone();
    ranked.sort_by_key(|(at, line)| {
        let last = match line {
            IndexLine::Note { name, .. } => {
                usage.notes.get(name.as_str()).map_or(0, |use_| use_.last)
            }
            IndexLine::Other(_) => 0,
        };
        std::cmp::Reverse((last, *at))
    });
    let mut bytes = 0usize;
    let mut keep: Vec<usize> = Vec::new();
    for (at, line) in ranked {
        let len = match line {
            IndexLine::Note { text, .. } | IndexLine::Other(text) => text.len().saturating_add(1),
        };
        if keep.len() >= LOAD_LINES || bytes.saturating_add(len) > LOAD_BYTES {
            continue;
        }
        bytes = bytes.saturating_add(len);
        keep.push(at);
    }
    let dropped = notes.len().saturating_sub(keep.len());
    let shown = notes
        .into_iter()
        .filter(|(at, _)| keep.contains(at))
        .map(|(_, line)| line)
        .collect();
    (shown, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::doc::draft;
    use crate::scratch::Scratch;

    fn temp(label: &str) -> Scratch {
        Scratch::new(&format!("yi-memstore-{label}")).unwrap()
    }

    fn note(name: &str, hook: &str) -> Memory {
        let text =
            format!("---\nname: {name}\ndescription: {hook}\ntype: project\n---\nbody of {name}\n");
        draft(&text, &[]).unwrap().memory
    }

    #[test]
    fn save_update_forget_keep_the_index_in_step() {
        let dir = temp("cycle");
        let store = Store::new(dir.join("store"));
        assert!(!store.save(note("alpha", "first hook")).unwrap().updated);
        assert!(!store.save(note("beta", "second hook")).unwrap().updated);
        assert!(store.save(note("alpha", "revised hook")).unwrap().updated);
        let index = fs::read_to_string(store.dir().join(INDEX)).unwrap();
        assert_eq!(
            index,
            "- [alpha](alpha.md) — revised hook\n- [beta](beta.md) — second hook\n"
        );
        assert_eq!(store.usage().notes.get("alpha").map(|u| u.saves), Some(2));
        store
            .forget(&MemoryName::from_stem("alpha").unwrap())
            .unwrap();
        let index = fs::read_to_string(store.dir().join(INDEX)).unwrap();
        assert_eq!(index, "- [beta](beta.md) — second hook\n");
        assert!(!store.dir().join("alpha.md").exists());
    }

    fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if String::from_utf8_lossy(&fs::read(&path).unwrap()).contains(needle) {
                    found.push(path);
                }
            }
        }
        found
    }

    fn ops(store: &Store) -> Vec<(String, String)> {
        let replay = store.journal().replay().unwrap();
        assert_eq!(replay.broken, None);
        replay
            .records
            .iter()
            .map(|record| (record.op.clone(), record.name.clone()))
            .collect()
    }

    #[test]
    fn a_forget_leaves_no_copy_of_any_version_in_the_store() {
        let dir = temp("forget");
        let store = Store::new(dir.join("store"));
        let secret = "the probe HOME sat under /tmp — 48 GB tmpfs, ✗ never again";
        let body = |extra: &str| {
            format!(
                "---\nname: tmp\ndescription: tmp is RAM\ntype: feedback\n---\n{secret}{extra}\n"
            )
        };
        store.save(draft(&body(""), &[]).unwrap().memory).unwrap();
        store
            .save(draft(&body(" (twice)"), &[]).unwrap().memory)
            .unwrap();
        store.save(note("keep", "kept hook")).unwrap();
        assert_eq!(
            files_holding(store.dir(), secret).len(),
            3,
            "two objects and the file"
        );
        store
            .forget(&MemoryName::from_stem("tmp").unwrap())
            .unwrap();
        assert_eq!(files_holding(store.dir(), secret), Vec::<PathBuf>::new());
        let report = store.rebuild().unwrap();
        assert_eq!((report.live, report.forgotten), (1, 1));
        assert!(!store.dir().join("tmp.md").exists());
        assert!(store.dir().join("keep.md").exists());
    }

    #[test]
    fn rebuild_restores_every_note_the_journal_holds() {
        let dir = temp("rebuild");
        let store = Store::new(dir.join("store"));
        store.save(note("alpha", "first hook")).unwrap();
        store.save(note("alpha", "revised hook")).unwrap();
        store.save(note("beta", "second hook")).unwrap();
        store
            .mark_read(&MemoryName::from_stem("beta").unwrap())
            .unwrap();
        let alpha = fs::read(store.dir().join("alpha.md")).unwrap();
        for file in ["alpha.md", "beta.md", INDEX, USAGE] {
            fs::remove_file(store.dir().join(file)).unwrap();
        }
        let report = store.rebuild().unwrap();
        assert_eq!((report.live, report.restored, report.forgotten), (2, 2, 0));
        assert_eq!(fs::read(store.dir().join("alpha.md")).unwrap(), alpha);
        let index = fs::read_to_string(store.dir().join(INDEX)).unwrap();
        assert_eq!(
            index,
            "- [alpha](alpha.md) — revised hook\n- [beta](beta.md) — second hook\n"
        );
        let usage = store.usage();
        assert_eq!(
            usage.notes.get("alpha").map(|u| (u.saves, u.reads)),
            Some((2, 0))
        );
        assert_eq!(
            usage.notes.get("beta").map(|u| (u.saves, u.reads)),
            Some((1, 1))
        );
    }

    #[test]
    fn rebuild_keeps_a_hand_edit_and_names_a_lost_object() {
        let dir = temp("differ");
        let store = Store::new(dir.join("store"));
        store.save(note("alpha", "hook")).unwrap();
        let hash = store.save(note("beta", "hook")).unwrap().hash;
        fs::write(store.dir().join("alpha.md"), "hand edit\n").unwrap();
        fs::remove_file(store.dir().join("beta.md")).unwrap();
        fs::remove_file(store.dir().join("objects").join(hash.hex())).unwrap();
        let report = store.rebuild().unwrap();
        assert_eq!(report.differ, vec!["alpha".to_owned()]);
        assert_eq!(report.missing, vec!["beta".to_owned()]);
        assert_eq!(
            fs::read_to_string(store.dir().join("alpha.md")).unwrap(),
            "hand edit\n"
        );
    }

    #[test]
    fn reconcile_journals_adopts_hand_edits_and_hand_deletions() {
        let dir = temp("adopt");
        let store = Store::new(dir.join("store"));
        fs::create_dir_all(store.dir()).unwrap();
        let old =
            "---\nname: old\ndescription: written before the journal\ntype: user\n---\nbody\n";
        fs::write(store.dir().join("old.md"), old).unwrap();
        store.reconcile().unwrap();
        store.reconcile().unwrap();
        assert_eq!(ops(&store), vec![("adopt".to_owned(), "old".to_owned())]);
        fs::write(store.dir().join("old.md"), format!("{old}more\n")).unwrap();
        store.reconcile().unwrap();
        fs::remove_file(store.dir().join("old.md")).unwrap();
        store.reconcile().unwrap();
        let replay = store.journal().replay().unwrap();
        let tail: Vec<&str> = replay
            .records
            .iter()
            .map(|record| record.op.as_str())
            .collect();
        assert_eq!(tail, vec!["adopt", "edit", "forget"]);
        let by = replay
            .records
            .last()
            .and_then(|record| record.extra.get("by"));
        assert_eq!(by, Some(&Value::from("hand")));
        assert_eq!(
            files_holding(store.dir(), "written before the journal"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn two_lanes_saving_at_once_keep_one_verified_chain() {
        let dir = temp("lanes");
        let lanes = [Store::new(dir.join("store")), Store::new(dir.join("store"))];
        std::thread::scope(|scope| {
            for i in 0..16 {
                let lane = &lanes[i % 2];
                scope.spawn(move || lane.save(note(&format!("n{i}"), "hook")).unwrap());
            }
        });
        let saves = ops(&lanes[0]);
        assert_eq!(saves.len(), 16);
        assert!(saves.iter().all(|(op, _)| op == "save"));
    }

    #[test]
    fn a_torn_last_line_is_cut_before_the_next_append() {
        let dir = temp("torn");
        let store = Store::new(dir.join("store"));
        store.save(note("alpha", "hook")).unwrap();
        let journal = store.dir().join("ops.jsonl");
        let mut text = fs::read_to_string(&journal).unwrap();
        text.push_str("{\"at\":1,\"na");
        fs::write(&journal, &text).unwrap();
        store.save(note("beta", "hook")).unwrap();
        assert_eq!(
            ops(&store),
            vec![
                ("save".to_owned(), "alpha".to_owned()),
                ("save".to_owned(), "beta".to_owned())
            ]
        );
    }

    #[test]
    fn a_hand_edited_index_line_survives_reconcile() {
        let dir = temp("hand");
        let store = Store::new(dir.join("store"));
        store.save(note("alpha", "model hook")).unwrap();
        store.save(note("beta", "beta hook")).unwrap();
        fs::write(
            store.dir().join(INDEX),
            "# Forge\n- [Alpha, my words](alpha.md) — the user's own hook\n- [gone](gone.md) — deleted by hand\n",
        )
        .unwrap();
        let lines = store.reconcile().unwrap();
        let index = fs::read_to_string(store.dir().join(INDEX)).unwrap();
        assert_eq!(
            index,
            "# Forge\n- [Alpha, my words](alpha.md) — the user's own hook\n- [beta](beta.md) — beta hook\n"
        );
        assert_eq!(lines.len(), 3);
        let found = store
            .resolve("the user's own hook")
            .map(|note| note.name.to_string());
        assert_eq!(found.as_deref(), Some("alpha"));
    }

    #[test]
    fn a_broken_frontmatter_still_indexes() {
        let dir = temp("broken");
        let store = Store::new(dir.join("store"));
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(
            store.dir().join("half.md"),
            "---\nname: half\ndescription: \"never closed\n---\nThe body's first line.\n",
        )
        .unwrap();
        let lines = store.reconcile().unwrap();
        assert_eq!(
            lines,
            vec![IndexLine::Note {
                name: MemoryName::from_stem("half").unwrap(),
                text: "- [half](half.md) — The body's first line.".to_owned()
            }]
        );
        let note = store.resolve("half").unwrap();
        assert_eq!(note.trouble.map(|t| t.line), Some(3));
    }

    #[test]
    fn an_unreadable_index_is_refused_and_left_alone() {
        let dir = temp("latin1");
        let store = Store::new(dir.join("store"));
        fs::create_dir_all(store.dir()).unwrap();
        let bytes = b"- [keep](keep.md) \x97 hand edit\n".to_vec();
        fs::write(store.dir().join(INDEX), &bytes).unwrap();
        let err = store.save(note("new", "hook")).unwrap_err();
        assert!(
            matches!(err, StoreError::Unreadable { ref file } if file == INDEX),
            "{err}"
        );
        assert!(store.reconcile().is_err());
        assert_eq!(fs::read(store.dir().join(INDEX)).unwrap(), bytes);
        assert!(!store.dir().join("new.md").exists());
    }

    #[test]
    fn concurrent_saves_keep_every_index_line() {
        let dir = temp("race");
        let store = Store::new(dir.join("store"));
        std::thread::scope(|scope| {
            for i in 0..16 {
                let store = &store;
                scope.spawn(move || store.save(note(&format!("n{i}"), "hook")).unwrap());
            }
        });
        let index = fs::read_to_string(store.dir().join(INDEX)).unwrap();
        assert_eq!(index.lines().count(), 16, "{index}");
        assert_eq!(store.usage().notes.len(), 16);
    }

    #[test]
    fn past_the_cap_the_recently_used_lines_load() {
        let lines: Vec<IndexLine> = (0..LOAD_LINES.saturating_add(5))
            .map(|i| IndexLine::Note {
                name: MemoryName::from_stem(&format!("n{i}")).unwrap(),
                text: format!("- [n{i}](n{i}.md) — hook"),
            })
            .collect();
        let mut usage = Usage::default();
        usage.notes.insert(
            "n0".to_owned(),
            NoteUsage {
                saves: 1,
                reads: 3,
                last: 99,
            },
        );
        let (shown, dropped) = loaded(&lines, &usage);
        assert_eq!(shown.len(), LOAD_LINES);
        assert_eq!(dropped, 5);
        assert_eq!(shown.first(), lines.first().as_ref());
        assert!(!shown.contains(&&lines[1]));
    }

    #[test]
    fn two_worktrees_share_one_directory() {
        let root = temp("wt");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let status = yi_tools::command("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.email=m@t",
                "-c",
                "user.name=m",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        git(&repo, &["worktree", "add", "-q", "../lane", "HEAD"]);
        let home = root.join("home");
        let a = repo_dir(&home, &repo);
        assert_eq!(a, repo_dir(&home, &root.join("lane")));
        let encoded = a
            .parent()
            .and_then(Path::file_name)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            encoded.starts_with("--") && encoded.ends_with("repo--"),
            "{encoded}"
        );
    }
}
