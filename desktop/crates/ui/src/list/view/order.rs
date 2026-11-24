//! The order a list draws its rows in: a flat list as its table has them, a tree parent
//! first with every child under its parent.

use std::collections::HashMap;

use yd_core::feed::Table;
use yd_core::ids::RowId;
use yd_core::row::{Row, Rows};

/// Where each drawn row is in its table, and how deep it sits under its parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::list) enum Order {
    /// Row `i` of the list is row `i` of the table.
    Flat,
    /// The table's rows walked parent first: a row's place in the table, and its depth.
    Tree(Vec<(usize, u8)>),
}

impl Order {
    pub(in crate::list) fn of(tree: bool, table: &Table) -> Self {
        if tree {
            Order::Tree(walk(table))
        } else {
            Order::Flat
        }
    }

    pub(in crate::list) fn len(&self, table: &Table) -> usize {
        match self {
            Order::Flat => table.len(),
            Order::Tree(slots) => slots.len(),
        }
    }

    /// The row drawn at `slot`, and its depth.
    pub(in crate::list) fn at<'a>(&self, table: &'a Table, slot: usize) -> Option<(&'a Row, u8)> {
        match self {
            Order::Flat => Some((table.get(slot)?, 0)),
            Order::Tree(slots) => {
                let (at, depth) = *slots.get(slot)?;
                Some((table.get(at)?, depth))
            }
        }
    }

    /// The slot of the row `id`: at `hint` if it still stands there, else found by a search.
    pub(in crate::list) fn slot_of(
        &self,
        table: &Table,
        id: &RowId,
        hint: Option<usize>,
    ) -> Option<usize> {
        let stands = |slot: &usize| self.at(table, *slot).is_some_and(|(row, _)| row.id == *id);
        hint.filter(stands)
            .or_else(|| (0..self.len(table)).find(stands))
    }
}

/// Every row once, parent first. A row whose parent is missing, or that hangs in a ring of
/// parents, is drawn as a root, so no row is ever lost.
fn walk(table: &Table) -> Vec<(usize, u8)> {
    let rows = || (0..table.len()).filter_map(|at| Some((at, table.get(at)?)));
    let places: HashMap<&RowId, usize> = rows().map(|(at, row)| (&row.id, at)).collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); table.len()];
    let mut roots = Vec::new();
    for (at, row) in rows() {
        let parent = row.parent.as_ref().and_then(|id| places.get(id));
        match parent.and_then(|parent| children.get_mut(*parent)) {
            Some(under) => under.push(at),
            None => roots.push(at),
        }
    }
    let mut seen = vec![false; table.len()];
    let mut order = Vec::with_capacity(table.len());
    let mut visit = |from: usize, seen: &mut Vec<bool>| {
        let mut stack = vec![(from, 0_u8)];
        while let Some((at, depth)) = stack.pop() {
            match seen.get_mut(at) {
                Some(done) if !*done => *done = true,
                Some(_) | None => continue,
            }
            order.push((at, depth));
            let under = children.get(at).map_or(&[][..], Vec::as_slice);
            let below = depth.saturating_add(1);
            stack.extend(under.iter().rev().map(|child| (*child, below)));
        }
    };
    for root in roots {
        visit(root, &mut seen);
    }
    for at in 0..table.len() {
        visit(at, &mut seen);
    }
    order
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::feed::{FeedEvent, Table};
    use yd_core::ids::RowId;
    use yd_core::row::Row;

    use super::Order;

    type Check = Result<(), Box<dyn Error>>;

    fn table(rows: &[(&str, Option<&str>)]) -> Table {
        let mut table = Table::default();
        let rows = rows.iter().map(|(id, parent)| {
            let mut row = Row::new(*id);
            row.parent = parent.map(RowId::new);
            row
        });
        table.apply(FeedEvent::Reset(rows.collect()));
        table
    }

    fn drawn(table: &Table) -> Vec<(String, u8)> {
        let order = Order::of(true, table);
        (0..order.len(table))
            .filter_map(|slot| order.at(table, slot))
            .map(|(row, depth)| (row.id.as_str().to_owned(), depth))
            .collect()
    }

    fn rows(pairs: &[(&str, u8)]) -> Vec<(String, u8)> {
        pairs
            .iter()
            .map(|(id, depth)| ((*id).to_owned(), *depth))
            .collect()
    }

    /// Record 5.3: depth first, each child directly under its parent, whenever it arrived.
    #[test]
    fn a_child_that_arrives_late_is_drawn_under_its_parent() -> Check {
        let late = table(&[
            ("root", None),
            ("fork", Some("root")),
            ("tests", Some("root")),
            ("docs", Some("root")),
            ("bisect", Some("tests")),
            ("second", None),
        ]);
        let want = [
            ("root", 0),
            ("fork", 1),
            ("tests", 1),
            ("bisect", 2),
            ("docs", 1),
            ("second", 0),
        ];
        assert_eq!(drawn(&late), rows(&want));
        Ok(())
    }

    /// A feed can name a parent that is gone, or two rows that name each other.
    #[test]
    fn a_row_with_no_parent_to_stand_under_is_still_drawn() -> Check {
        let broken = table(&[("orphan", Some("gone")), ("a", Some("b")), ("b", Some("a"))]);
        assert_eq!(drawn(&broken), rows(&[("orphan", 0), ("a", 0), ("b", 1)]));
        Ok(())
    }

    #[test]
    fn a_row_is_found_where_it_stood_or_by_a_search() -> Check {
        let flat = table(&[("a", None), ("b", None), ("c", None)]);
        let order = Order::of(false, &flat);
        let id = RowId::new("c");
        assert_eq!(order.slot_of(&flat, &id, Some(2)), Some(2));
        assert_eq!(order.slot_of(&flat, &id, Some(0)), Some(2), "a stale hint");
        assert_eq!(order.slot_of(&flat, &id, None), Some(2));
        assert_eq!(order.slot_of(&flat, &RowId::new("gone"), Some(1)), None);
        Ok(())
    }
}
