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
        if let Some(old) = old.filter(|old| old.id != id) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_image_under_the_same_id_is_not_deleted_after_its_paint() {
        let rect = Rect::new(0, 0, 10, 5);
        let mut placed = BTreeMap::new();
        let mut out = Vec::new();
        for seq in [1, 2] {
            let want = BTreeMap::from([((), (rect, seq, "png"))]);
            reconcile(
                &mut out,
                &want,
                &mut placed,
                |_, _| 7,
                |out, id, _, _| write!(out, "P{id};").is_ok(),
                |out, id| {
                    let _ = write!(out, "D{id};");
                },
            );
        }
        assert_eq!(String::from_utf8_lossy(&out), "P7;P7;");
    }
}
