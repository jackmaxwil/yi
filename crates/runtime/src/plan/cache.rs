//! A journal reduced once per change: every reader of an unchanged `ops.jsonl` shares one
//! reduction instead of re-reading, re-verifying and re-applying the whole chain.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use yi_types::plan::doc::PlanId;
use yi_types::plan::ledger::Seq;

use super::journal::{Damage, Journal, Reading};
use super::state::{RootState, reduce};
use super::store::StoreError;

pub struct Reduced {
    pub reading: Reading,
    pub state: RootState,
}

/// Invariant: a journal changes only by append, truncation or replacement, so a file whose
/// device, inode, length, mtime and last line all match holds the bytes it held.
#[derive(PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    tail: Vec<u8>,
}

type Slot = Arc<Mutex<Option<(Stamp, Arc<Reduced>)>>>;

// ponytail: one slot per journal path for the process's life; evict when roots number thousands.
static SLOTS: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();

fn stamp(journal: &Journal) -> Result<Option<Stamp>, StoreError> {
    let Ok(meta) = std::fs::metadata(journal.path()) else {
        return Ok(None);
    };
    Ok(journal.tail()?.map(|tail| Stamp {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime: (meta.mtime(), meta.mtime_nsec()),
        tail,
    }))
}

/// A damaged chain is refused here, so a reading handed back is whole or torn at its tail.
fn fresh(journal: &Journal, root: &PlanId) -> Result<Reduced, StoreError> {
    let reading = journal.read()?;
    if let Some(Damage::Corrupt {
        offset,
        seq,
        reason,
        ..
    }) = reading.damage
    {
        return Err(StoreError::RecoveryRequired {
            root: root.clone(),
            seq: seq.map(Seq::get),
            offset,
            reason,
        });
    }
    let state = reduce(&reading.records).map_err(|source| StoreError::Reduce {
        root: root.clone(),
        source,
    })?;
    Ok(Reduced { reading, state })
}

/// Invariant: the stamp is taken before the read, so a write racing the read can only leave
/// a stamp older than the bytes, which the next reader sees as a change, never the reverse.
pub fn reduced(journal: &Journal, root: &PlanId) -> Result<Arc<Reduced>, StoreError> {
    let Some(stamp) = stamp(journal)? else {
        return fresh(journal, root).map(Arc::new);
    };
    let slot = {
        let mut slots = SLOTS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(slots.entry(journal.path().to_path_buf()).or_default())
    };
    let mut held = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((seen, reduced)) = held.as_ref()
        && *seen == stamp
    {
        return Ok(Arc::clone(reduced));
    }
    let reduced = Arc::new(fresh(journal, root)?);
    *held = Some((stamp, Arc::clone(&reduced)));
    Ok(reduced)
}
