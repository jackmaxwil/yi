use ratatui::layout::Rect;
use std::collections::BTreeMap;

/// (rect, payload frame, kitty id) of one placement that is currently on screen.
pub type Placed = (Rect, u64, u32);

/// One reconcile loop for every kitty placement: a want that vanished deletes its id, an
/// unchanged (rect, frame) pair is skipped, and a paint that lands records its id and
/// deletes the one it replaced.
pub fn reconcile<K: Ord + Copy, P>(
    want: impl IntoIterator<Item = (K, (Rect, u64, P))>,
    placed: &mut BTreeMap<K, Placed>,
    next_id: &mut impl FnMut(K, Option<u32>) -> u32,
    paint: &mut impl FnMut(K, Rect, &P, u32) -> bool,
    delete: &mut impl FnMut(K, u32),
) {
    let want: BTreeMap<K, (Rect, u64, P)> = want.into_iter().collect();
    let stale: Vec<K> = placed
        .keys()
        .filter(|key| !want.contains_key(*key))
        .copied()
        .collect();
    for key in stale {
        if let Some((_, _, id)) = placed.remove(&key) {
            delete(key, id);
        }
    }
    for (key, (rect, frame, payload)) in want {
        let old = placed.get(&key).copied();
        if old.is_some_and(|old| old.0 == rect && old.1 == frame) {
            continue;
        }
        let id = next_id(key, old.map(|old| old.2));
        if paint(key, rect, &payload, id) {
            if let Some(old) = old {
                delete(key, old.2);
            }
            placed.insert(key, (rect, frame, id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vanished want deletes its id, and an unchanged pair paints nothing; a failed
    /// paint keeps the id already on screen.
    #[test]
    fn reconcile_deletes_the_vanished_and_skips_the_unchanged() {
        let mut placed = BTreeMap::from([(1, (Rect::new(0, 0, 4, 4), 7, 11))]);
        let mut painted = 0;
        let mut deleted = Vec::new();
        reconcile(
            Some((1, (Rect::new(0, 0, 4, 4), 7u64, 9i32))),
            &mut placed,
            &mut |_, old| old.map_or(3, |old| old + 1),
            &mut |_, _, _, _| {
                painted += 1;
                true
            },
            &mut |_, id| deleted.push(id),
        );
        assert_eq!(painted, 0, "an unchanged pair is not repainted");
        reconcile(
            None::<(i32, (Rect, u64, i32))>,
            &mut placed,
            &mut |_, _| 0,
            &mut |_, _, _, _| true,
            &mut |_, id| deleted.push(id),
        );
        assert_eq!(deleted, vec![11]);
        assert!(placed.is_empty());
    }
}
