//! A journal reduced once per change: every reader of an unchanged `ops.jsonl` shares one
//! reduction instead of re-reading, re-verifying and re-applying the whole chain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use yi_types::plan::doc::PlanId;
use yi_types::plan::ledger::Seq;

use super::state::{RootState, reduce};
use super::store::StoreError;
use crate::file_cache::{Slot, Stamp};
use crate::journal::{Damage, Journal, Reading};

pub(crate) struct Reduced {
    pub(crate) reading: Reading,
    pub(crate) state: RootState,
}

// ponytail: one slot per journal path for the process's life; evict when roots number thousands.
static SLOTS: OnceLock<Mutex<HashMap<PathBuf, Arc<Slot<Reduced>>>>> = OnceLock::new();

fn stamp(journal: &Journal) -> Result<Option<Stamp>, StoreError> {
    Ok(Stamp::of(journal.path(), journal.tail()?))
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
pub(crate) fn reduced(journal: &Journal, root: &PlanId) -> Result<Arc<Reduced>, StoreError> {
    let Some(stamp) = stamp(journal)? else {
        return fresh(journal, root).map(Arc::new);
    };
    let slot = {
        let mut slots = SLOTS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(slots.entry(journal.path().to_path_buf()).or_default())
    };
    slot.load(stamp, || fresh(journal, root))
}
