//! An unchanged journal shares one settled reduction; concurrent cold loads may overlap, and
//! cache hits never wait for a fresh read, verification and reduction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

const SLOT_CAP: usize = 128;
static SLOTS: OnceLock<Mutex<HashMap<PathBuf, Arc<Slot<Reduced>>>>> = OnceLock::new();

fn take_slot(slots: &mut HashMap<PathBuf, Arc<Slot<Reduced>>>, path: &Path) -> Arc<Slot<Reduced>> {
    if slots.len() >= SLOT_CAP
        && !slots.contains_key(path)
        && let Some(oldest) = slots.keys().next().cloned()
    {
        slots.remove(&oldest);
    }
    Arc::clone(slots.entry(path.to_path_buf()).or_default())
}

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
        take_slot(&mut slots, journal.path())
    };
    if let Some(cached) = slot.cached(&stamp) {
        return Ok(cached);
    }
    let cached = Arc::new(fresh(journal, root)?);
    Ok(slot.store(stamp, cached))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_slot_map_stays_bounded() {
        let mut slots = HashMap::new();
        for n in 0..=SLOT_CAP {
            let path = PathBuf::from(format!("plan-{n}"));
            take_slot(&mut slots, &path);
        }
        assert_eq!(slots.len(), SLOT_CAP);
        assert!(slots.contains_key(Path::new("plan-128")));
    }
}
