//! `ops.jsonl`, the plan journal (plan section 5.3): one canonical record per line, digest chained.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::plan::canonical::{CanonicalError, Digest};
use yi_types::plan::ledger::{JournalRecord, Seq};

/// Bytes per record, newline included; a transaction that would exceed it is refused first.
pub const RECORD_CAP: usize = 64 * 1024;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        yi_session::now_ms()
    }
}

pub trait Fs: Send + Sync {
    fn write_all(&self, file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()>;
    fn sync_data(&self, file: &std::fs::File) -> std::io::Result<()>;

    /// The whole-file sync behind a checkpoint or an artifact and the directory naming it.
    fn sync_all(&self, file: &std::fs::File) -> std::io::Result<()> {
        file.sync_all()
    }
}

pub struct RealFs;

impl Fs for RealFs {
    fn write_all(&self, file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
        file.write_all(bytes)
    }

    fn sync_data(&self, file: &std::fs::File) -> std::io::Result<()> {
        file.sync_data()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: journal sync failed, the record is not acknowledged: {source}")]
    Sync {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("journal record of {bytes} bytes exceeds the {cap} byte cap; nothing was written")]
    RecordOverCap { bytes: usize, cap: usize },
    #[error("journal record: {0}")]
    Canonical(#[from] CanonicalError),
    #[error("journal {path} has a torn or damaged tail; set it aside before appending")]
    TailNotClean { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    TornTail {
        offset: u64,
        bytes: Vec<u8>,
    },
    Corrupt {
        offset: u64,
        seq: Option<Seq>,
        reason: String,
        resync: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub records: Vec<JournalRecord>,
    pub damage: Option<Damage>,
}

impl Reading {
    pub fn last(&self) -> Option<&JournalRecord> {
        self.records.last()
    }

    pub fn mark(&self) -> Option<(Seq, Digest)> {
        self.last().map(|record| (record.seq, record.digest))
    }
}

#[derive(Clone)]
pub struct Journal {
    path: PathBuf,
    fs: Arc<dyn Fs>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Journal")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> JournalError + '_ {
    move |source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn decode(
    line: &[u8],
    expected: Seq,
    prev: Option<&Digest>,
) -> Result<JournalRecord, (Option<Seq>, String)> {
    let record: JournalRecord =
        serde_json::from_slice(line).map_err(|error| (None, error.to_string()))?;
    let seq = record.seq;
    if seq != expected {
        return Err((
            Some(seq),
            format!("seq {seq} where {expected} was expected"),
        ));
    }
    let digest = record
        .digest_of(prev)
        .map_err(|error| (Some(seq), error.to_string()))?;
    if digest != record.digest {
        return Err((
            Some(seq),
            format!("digest {} does not cover the record's bytes", record.digest),
        ));
    }
    Ok(record)
}

fn resync<R: Read>(reader: &mut R, mut offset: u64) -> std::io::Result<Option<u64>> {
    let mut chunk = [0u8; 4096];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        if let Some(at) = chunk
            .get(..read)
            .and_then(|got| got.iter().position(|b| *b == b'\n'))
        {
            let after = u64::try_from(at).unwrap_or(u64::MAX).saturating_add(1);
            return Ok(Some(offset.saturating_add(after)));
        }
        offset = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
}

impl Journal {
    pub fn open(path: PathBuf, fs: Arc<dyn Fs>) -> Self {
        Self { path, fs }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Reading, JournalError> {
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Reading {
                    records: Vec::new(),
                    damage: None,
                });
            }
            Err(source) => return Err(io_at(&self.path)(source)),
        };
        let mut reader = BufReader::new(file);
        let mut records: Vec<JournalRecord> = Vec::new();
        let mut offset = 0u64;
        let cap = u64::try_from(RECORD_CAP).unwrap_or(u64::MAX);
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(cap.saturating_add(1))
                .read_until(b'\n', &mut line)
                .map_err(io_at(&self.path))?;
            if read == 0 {
                return Ok(Reading {
                    records,
                    damage: None,
                });
            }
            let terminated = line.last() == Some(&b'\n');
            let over = read > RECORD_CAP;
            if !terminated && !over {
                return Ok(Reading {
                    records,
                    damage: Some(Damage::TornTail {
                        offset,
                        bytes: line,
                    }),
                });
            }
            let expected = records
                .last()
                .map_or(Ok(Seq::FIRST), |last| last.seq.next())
                .map_err(|error| {
                    JournalError::Canonical(CanonicalError::Serialize {
                        detail: error.to_string(),
                    })
                })?;
            let prev = records.last().map(|last| &last.digest);
            let decoded = if over {
                Err((None, format!("record exceeds the {RECORD_CAP} byte cap")))
            } else {
                decode(
                    line.get(..read.saturating_sub(1)).unwrap_or(&[]),
                    expected,
                    prev,
                )
            };
            match decoded {
                Ok(record) => {
                    records.push(record);
                    offset = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                }
                Err((seq, reason)) => {
                    let after = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                    let resync = if terminated {
                        Some(after)
                    } else {
                        resync(&mut reader, after).map_err(io_at(&self.path))?
                    };
                    return Ok(Reading {
                        records,
                        damage: Some(Damage::Corrupt {
                            offset,
                            seq,
                            reason,
                            resync,
                        }),
                    });
                }
            }
        }
    }

    pub fn seal(
        &self,
        mut draft: JournalRecord,
        prev: Option<&JournalRecord>,
    ) -> Result<Sealed, JournalError> {
        draft.seq = match prev {
            Some(prev) => prev.seq.next().map_err(|error| {
                JournalError::Canonical(CanonicalError::Serialize {
                    detail: error.to_string(),
                })
            })?,
            None => Seq::FIRST,
        };
        let record = draft.seal(prev.map(|prev| &prev.digest))?;
        let line = record.line()?;
        if line.len() > RECORD_CAP {
            return Err(JournalError::RecordOverCap {
                bytes: line.len(),
                cap: RECORD_CAP,
            });
        }
        Ok(Sealed { record, line })
    }

    /// A dry seal before any effect: the line must fit with `headroom` bytes to spare, the
    /// bound on what the caller adds after its effects; the seal at commit is the backstop.
    pub fn rehearse(
        &self,
        draft: JournalRecord,
        prev: Option<&JournalRecord>,
        headroom: usize,
    ) -> Result<(), JournalError> {
        let sealed = self.seal(draft, prev)?;
        let bytes = sealed.line.len().saturating_add(headroom);
        if bytes > RECORD_CAP {
            return Err(JournalError::RecordOverCap {
                bytes,
                cap: RECORD_CAP,
            });
        }
        Ok(())
    }

    /// The commit point: append one line, then `sync_data`, under the lease, past a clean tail.
    /// Invariant: a first record that fails leaves no empty journal to list a root with no plan.
    pub fn append(&self, sealed: &Sealed) -> Result<(), JournalError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io_at(parent))?;
        }
        let created = !self.path.exists();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .map_err(io_at(&self.path))?;
        let written = self
            .fs
            .write_all(&mut file, &sealed.line)
            .map_err(io_at(&self.path))
            .and_then(|()| {
                self.fs
                    .sync_data(&file)
                    .map_err(|source| JournalError::Sync {
                        path: self.path.clone(),
                        source,
                    })
            });
        if written.is_err() && created {
            drop(file);
            let _removed_best_effort = std::fs::remove_file(&self.path);
        }
        written
    }

    pub fn set_aside(&self, offset: u64, nonce: &str) -> Result<PathBuf, JournalError> {
        let bytes = std::fs::read(&self.path).map_err(io_at(&self.path))?;
        let at = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let kept = bytes.get(at..).unwrap_or(&[]);
        let side = self.path.with_extension(format!("torn.{nonce}"));
        std::fs::write(&side, kept).map_err(io_at(&side))?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(io_at(&self.path))?;
        file.set_len(offset).map_err(io_at(&self.path))?;
        file.sync_all().map_err(io_at(&self.path))?;
        Ok(side)
    }
}

/// A journal with a record in it; an empty file is a first append that never landed.
pub fn has_record(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

#[must_use]
#[derive(Debug, Clone)]
pub struct Sealed {
    pub record: JournalRecord,
    pub line: Vec<u8>,
}
