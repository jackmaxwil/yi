use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write};
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

    pub fn replay(&self) -> Result<Replay, StoreError> {
        let text = match fs::read(self.dir.join(JOURNAL)) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Replay::default()),
            Err(error) => return Err(io(JOURNAL)(error)),
        };
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut replay = Replay::default();
        for (at, line) in lines.iter().enumerate() {
            let last = at.saturating_add(1) == lines.len();
            match serde_json::from_str::<MemoryRecord>(line.trim_end()) {
                Ok(record) if line.ends_with('\n') => {
                    let prev = replay.records.last().map(|record| &record.digest);
                    let chained = record.digest_of(prev).ok() == Some(record.digest);
                    if !chained && replay.broken.is_none() {
                        replay.broken = Some(at.saturating_add(1));
                    }
                    replay.records.push(record);
                }
                _ if last => {
                    let prefix = text
                        .get(..text.len().saturating_sub(line.len()))
                        .unwrap_or_default();
                    self.replace(JOURNAL, prefix)?;
                }
                _ => {
                    replay.broken.get_or_insert(at.saturating_add(1));
                }
            }
        }
        Ok(replay)
    }

    fn replace(&self, file: &str, text: &str) -> Result<(), StoreError> {
        yi_session::replace_file(&self.dir.join(file), text.as_bytes()).map_err(io(file))
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
        let record = unsealed
            .seal(prev.as_ref())
            .map_err(|error| StoreError::Journal {
                detail: format!("{error:?}"),
            })?;
        let line = record.line().map_err(|error| StoreError::Journal {
            detail: format!("{error:?}"),
        })?;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(JOURNAL))
            .and_then(|mut file| file.write_all(&line))
            .map_err(io(JOURNAL))?;
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
