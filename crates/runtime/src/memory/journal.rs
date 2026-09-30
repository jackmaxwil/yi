use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use yi_types::memory::MemoryRecord;
use yi_types::plan::canonical::{Chained, Digest};

use super::store::StoreError;

pub const SAVE: &str = "save";
pub const EDIT: &str = "edit";
pub const ADOPT: &str = "adopt";
pub const FORGET: &str = "forget";
pub const READ: &str = "read";

const JOURNAL: &str = "ops.jsonl";
const OBJECTS: &str = "objects";

fn io(file: &str) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        file: file.to_owned(),
        source,
    }
}

fn canonical(error: impl std::fmt::Debug) -> StoreError {
    StoreError::Journal {
        detail: format!("{error:?}"),
    }
}

fn journal(error: crate::plan::journal::JournalError) -> StoreError {
    match error {
        crate::plan::journal::JournalError::Io { source, .. }
        | crate::plan::journal::JournalError::Sync { source, .. } => io(JOURNAL)(source),
        other => canonical(other),
    }
}

#[derive(Debug, Default)]
pub struct Replay {
    pub records: Vec<MemoryRecord>,
    pub broken: Option<usize>,
}

impl Replay {
    pub fn heads(&self) -> BTreeMap<&str, &MemoryRecord> {
        let mut heads = BTreeMap::new();
        for record in self.records.iter().filter(|record| record.op != READ) {
            heads.insert(record.name.as_str(), record);
        }
        heads
    }

    fn versions<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Digest> + 'a {
        self.records
            .iter()
            .filter(move |record| record.name == name && record.op != FORGET)
            .map(|record| &record.hash)
    }
}

pub struct Journal<'a> {
    dir: &'a Path,
}

impl<'a> Journal<'a> {
    pub fn new(dir: &'a Path) -> Self {
        Self { dir }
    }

    fn object_path(&self, hash: &Digest) -> PathBuf {
        self.dir.join(OBJECTS).join(hash.hex())
    }

    fn log(&self) -> crate::plan::journal::Journal {
        let fs = std::sync::Arc::new(crate::plan::journal::RealFs);
        crate::plan::journal::Journal::open(self.dir.join(JOURNAL), fs)
    }

    /// A line that is not a record marks the journal broken and replay reads on past it; a torn
    /// last line is set aside beside the journal with its bytes kept.
    pub fn replay(&self) -> Result<Replay, StoreError> {
        let log = self.log();
        let mut lines = log.lines().map_err(journal)?;
        // An unterminated run past the cap is torn too: the next append would glue onto it.
        if lines.lines.last().is_some_and(|line| line.next.is_none()) {
            let run = lines.lines.pop().map(|line| line.offset);
            lines.torn = lines.torn.or(run.map(|offset| (offset, Vec::new())));
        }
        let mut replay = Replay::default();
        for (at, line) in lines.lines.iter().enumerate() {
            let record = line
                .bytes
                .as_ref()
                .ok()
                .and_then(|bytes| serde_json::from_slice::<MemoryRecord>(bytes).ok());
            let Some(record) = record else {
                replay.broken.get_or_insert(at.saturating_add(1));
                continue;
            };
            let prev = replay.records.last().map(|record| &record.digest);
            if record.digest_of(prev).ok() != Some(record.digest) {
                replay.broken.get_or_insert(at.saturating_add(1));
            }
            replay.records.push(record);
        }
        if let Some((offset, _)) = lines.torn {
            log.set_aside(offset, &yi_session::nonce())
                .map_err(journal)?;
        }
        Ok(replay)
    }

    pub fn append(
        &self,
        op: &str,
        name: &str,
        hash: Digest,
        session: Option<&str>,
        extra: Map<String, Value>,
    ) -> Result<MemoryRecord, StoreError> {
        let replay = self.replay()?;
        let prev = replay.records.last().map(|record| record.digest);
        let unsealed = MemoryRecord {
            op: op.to_owned(),
            name: name.to_owned(),
            hash,
            at: super::store::now(),
            session: session.map(str::to_owned),
            extra,
            digest: Digest::of(b""),
        };
        let record = unsealed.seal(prev.as_ref()).map_err(canonical)?;
        let line = record.line().map_err(canonical)?;
        self.log().append_line(&line).map_err(journal)?;
        Ok(record)
    }

    pub fn put_object(&self, text: &str) -> Result<Digest, StoreError> {
        let hash = Digest::of(text.as_bytes());
        let path = self.object_path(&hash);
        if path.is_file() {
            return Ok(hash);
        }
        fs::create_dir_all(self.dir.join(OBJECTS))
            .and_then(|()| yi_session::replace_file(&path, text.as_bytes()))
            .map_err(io(OBJECTS))?;
        Ok(hash)
    }

    pub fn object(&self, hash: &Digest) -> Option<String> {
        fs::read_to_string(self.object_path(hash)).ok()
    }

    pub fn drop_objects(&self, replay: &Replay, name: &str) -> Result<(), StoreError> {
        for hash in replay.versions(name) {
            match fs::remove_file(self.object_path(hash)) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(io(OBJECTS)(error)),
            }
        }
        Ok(())
    }
}
