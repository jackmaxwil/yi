use std::collections::BTreeMap;
use std::io::Write;

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub rect: Rect,
    pub seq: u64,
    pub id: u32,
}

pub fn reconcile<K, W>(
    out: &mut W,
    want: &BTreeMap<K, (Rect, u64, &str)>,
    placed: &mut BTreeMap<K, Placed>,
    next_id: impl Fn(K, Option<Placed>) -> u32,
    paint: impl Fn(&mut W, u32, Rect, &str) -> bool,
    delete: impl Fn(&mut W, u32),
) where
    K: Ord + Copy,
    W: Write,
{
    placed.retain(|slot, at| {
        let keep = want.contains_key(slot);
        if !keep {
            delete(out, at.id);
        }
        keep
    });
    for (slot, (rect, seq, png)) in want {
        let old = placed.get(slot).copied();
        if old.is_some_and(|old| old.rect == *rect && old.seq == *seq) {
            continue;
        }
        let id = next_id(*slot, old);
        if !paint(out, id, *rect, png) {
            continue;
        }
        if let Some(old) = old {
            delete(out, old.id);
        }
        placed.insert(
            *slot,
            Placed {
                rect: *rect,
                seq: *seq,
                id,
            },
        );
    }
}
