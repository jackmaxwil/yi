//! BSP tree layout: panes tile by recursive ratio splits, navigation is
//! geometric over the computed rects.

use std::cmp::Reverse;

use ratatui::layout::{Direction, Margin, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u32);

impl PaneId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// A framed pane keeps one blank column between its border and its text; a terminal's grid fills its box.
pub(crate) fn pane_margin(framed: bool, fills: bool) -> Margin {
    Margin::new(if framed && !fills { 2 } else { 1 }, u16::from(framed))
}

pub(crate) fn split_off_bottom(area: Rect, height: u16) -> (Rect, Rect) {
    let height = height.min(area.height);
    let top = Rect {
        height: area.height.saturating_sub(height),
        ..area
    };
    let bottom = Rect {
        y: area.y.saturating_add(top.height),
        height,
        ..area
    };
    (top, bottom)
}

pub(crate) fn split_off_top(area: Rect, height: u16) -> (Rect, Rect) {
    let height = height.min(area.height);
    let top = Rect { height, ..area };
    let rest = Rect {
        y: area.y.saturating_add(height),
        height: area.height.saturating_sub(height),
        ..area
    };
    (top, rest)
}

/// Allocator lives on the tab so ids stay deterministic under drive scripts.
#[derive(Debug, Default)]
pub struct PaneIds(u32);

impl PaneIds {
    pub fn allocate(&mut self) -> PaneId {
        self.0 = self.0.wrapping_add(1);
        PaneId(self.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaneRect {
    pub id: PaneId,
    pub rect: Rect,
    pub focused: bool,
}

/// A split boundary, addressable by tree path for drag resize.
#[derive(Debug, Clone)]
pub struct SplitBorder {
    pub pos: u16,
    pub direction: Direction,
    pub area: Rect,
    pub path: Vec<bool>,
}

#[derive(Debug, Clone, Copy)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Where a panel lands on the screen, whatever the tiling around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Left,
    Right,
    Top,
    Bottom,
    LeftThird,
    MiddleThird,
    RightThird,
}

pub enum Node {
    Pane(PaneId),
    Split {
        direction: Direction,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

pub struct TileLayout {
    root: Node,
    focus: PaneId,
    prev_focus: Option<PaneId>,
}

impl TileLayout {
    pub fn new(ids: &mut PaneIds) -> (Self, PaneId) {
        let root = ids.allocate();
        (
            Self {
                root: Node::Pane(root),
                focus: root,
                prev_focus: None,
            },
            root,
        )
    }

    fn set_focus(&mut self, id: PaneId) {
        if id != self.focus {
            self.prev_focus = Some(self.focus);
            self.focus = id;
        }
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn pane_count(&self) -> usize {
        self.pane_ids().len()
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        collect_ids(&self.root, &mut ids);
        ids
    }

    pub fn panes(&self, area: Rect) -> Vec<PaneRect> {
        let mut result = Vec::new();
        collect_panes(&self.root, area, self.focus, &mut result);
        result
    }

    pub fn splits(&self, area: Rect) -> Vec<SplitBorder> {
        let mut result = Vec::new();
        collect_splits(&self.root, area, Vec::new(), &mut result);
        result
    }

    /// Split the focused pane; the new pane takes focus.
    pub fn split_focused(&mut self, ids: &mut PaneIds, direction: Direction) -> PaneId {
        let new_id = ids.allocate();
        let target = self.focus;
        let old = std::mem::replace(&mut self.root, Node::Pane(new_id));
        self.root = split_at(old, target, direction, new_id, 0.5);
        self.set_focus(new_id);
        new_id
    }

    /// Close the focused pane; false when it is the last one.
    pub fn close_focused(&mut self) -> Option<PaneId> {
        let ids = self.pane_ids();
        if ids.len() <= 1 {
            return None;
        }
        let target = self.focus;
        let pos = ids.iter().position(|id| *id == target)?;
        let ordered = ids
            .get(pos.saturating_add(1))
            .or_else(|| ids.get(pos.saturating_sub(1)))
            .copied()?;
        let new_focus = match self.prev_focus {
            Some(prev) if prev != target && ids.contains(&prev) => prev,
            _ => ordered,
        };
        let old = std::mem::replace(&mut self.root, Node::Pane(target));
        let new_root = remove_pane(old, target)?;
        self.root = new_root;
        self.focus = new_focus;
        self.prev_focus = None;
        Some(target)
    }

    /// Take `id` out of the tree and put it on the screen's `snap` edge, the panels that were
    /// there keeping their arrangement beside it; false when it is the only one.
    pub fn snap(&mut self, id: PaneId, snap: Snap) -> bool {
        if self.pane_count() <= 1 || !self.pane_ids().contains(&id) {
            return false;
        }
        let old = std::mem::replace(&mut self.root, Node::Pane(id));
        let Some(rest) = remove_pane(old, id) else {
            self.root = Node::Pane(id);
            return false;
        };
        self.root = place(rest, id, snap);
        self.set_focus(id);
        true
    }

    /// A new panel on the screen's `snap` edge; it takes focus.
    pub fn snap_new(&mut self, ids: &mut PaneIds, snap: Snap) -> PaneId {
        let id = ids.allocate();
        let old = std::mem::replace(&mut self.root, Node::Pane(id));
        self.root = place(old, id, snap);
        self.set_focus(id);
        id
    }

    pub fn focus_pane(&mut self, id: PaneId) {
        if self.pane_ids().contains(&id) {
            self.set_focus(id);
        }
    }

    /// Move focus geometrically; false when no pane lies that way.
    pub fn focus_direction(&mut self, direction: NavDirection, area: Rect) -> bool {
        let panes = self.panes(area);
        let Some(focused) = panes.iter().find(|pane| pane.focused).copied() else {
            return false;
        };
        match find_in_direction(focused, direction, &panes) {
            Some(id) => {
                self.set_focus(id);
                true
            }
            None => false,
        }
    }

    pub fn set_ratio_at(&mut self, path: &[bool], ratio: f32) -> bool {
        set_ratio_at(&mut self.root, path, valid_ratio(ratio))
    }

    /// Path to the split whose second child is `id` — the split a fresh
    /// `split_focused` created, which is what an open animation eases.
    pub fn split_path_of_second(&self, id: PaneId) -> Option<Vec<bool>> {
        fn walk(node: &Node, id: PaneId, path: &mut Vec<bool>) -> bool {
            match node {
                Node::Pane(_) => false,
                Node::Split { first, second, .. } => {
                    if matches!(**second, Node::Pane(pane) if pane == id) {
                        return true;
                    }
                    path.push(false);
                    if walk(first, id, path) {
                        return true;
                    }
                    path.pop();
                    path.push(true);
                    if walk(second, id, path) {
                        return true;
                    }
                    path.pop();
                    false
                }
            }
        }
        let mut path = Vec::new();
        walk(&self.root, id, &mut path).then_some(path)
    }

    pub fn ratio_at(&self, path: &[bool]) -> Option<f32> {
        ratio_at(&self.root, path)
    }
}

fn find_in_direction(
    focused: PaneRect,
    direction: NavDirection,
    panes: &[PaneRect],
) -> Option<PaneId> {
    let fr = focused.rect;
    panes
        .iter()
        .enumerate()
        .filter(|(_, pane)| pane.id != focused.id)
        .filter(|(_, pane)| {
            let r = pane.rect;
            match direction {
                // The one-cell border overlap counts as adjacency, not
                // containment, so each edge test tolerates it.
                NavDirection::Left => {
                    r.x.saturating_add(r.width) <= fr.x.saturating_add(1)
                        && overlaps(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Right => {
                    r.x.saturating_add(1) >= fr.x.saturating_add(fr.width)
                        && overlaps(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Up => {
                    r.y.saturating_add(r.height) <= fr.y.saturating_add(1)
                        && overlaps(r.x, r.width, fr.x, fr.width)
                }
                NavDirection::Down => {
                    r.y.saturating_add(1) >= fr.y.saturating_add(fr.height)
                        && overlaps(r.x, r.width, fr.x, fr.width)
                }
            }
        })
        .min_by_key(|(index, pane)| {
            let r = pane.rect;
            let edge = match direction {
                NavDirection::Left => fr.x.saturating_sub(r.x.saturating_add(r.width)),
                NavDirection::Right => r.x.saturating_sub(fr.x.saturating_add(fr.width)),
                NavDirection::Up => fr.y.saturating_sub(r.y.saturating_add(r.height)),
                NavDirection::Down => r.y.saturating_sub(fr.y.saturating_add(fr.height)),
            };
            let overlap = match direction {
                NavDirection::Left | NavDirection::Right => {
                    overlap_amount(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Up | NavDirection::Down => {
                    overlap_amount(r.x, r.width, fr.x, fr.width)
                }
            };
            (edge, Reverse(overlap), *index)
        })
        .map(|(_, pane)| pane.id)
}

fn overlaps(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_start < b_end && a_end > b_start
}

fn overlap_amount(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> u16 {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_end.min(b_end).saturating_sub(a_start.max(b_start))
}

const THIRD: f32 = 1.0 / 3.0;

fn halves(direction: Direction, ratio: f32, first: Node, second: Node) -> Node {
    Node::Split {
        direction,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// `ids` stacked top to bottom in equal rows; `None` for no panels.
fn column(ids: &[PaneId]) -> Option<Node> {
    let (first, rest) = ids.split_first()?;
    let Some(below) = column(rest) else {
        return Some(Node::Pane(*first));
    };
    let share = 1.0 / f32::from(u16::try_from(ids.len()).unwrap_or(u16::MAX));
    Some(halves(
        Direction::Vertical,
        share,
        Node::Pane(*first),
        below,
    ))
}

/// `rest` with `id` on the `snap` edge. The middle third restacks the others into a column each
/// side of it, first half left; a lone other panel takes the left third.
fn place(rest: Node, id: PaneId, snap: Snap) -> Node {
    let this = Node::Pane(id);
    match snap {
        Snap::Left => halves(Direction::Horizontal, 0.5, this, rest),
        Snap::Right => halves(Direction::Horizontal, 0.5, rest, this),
        Snap::Top => halves(Direction::Vertical, 0.5, this, rest),
        Snap::Bottom => halves(Direction::Vertical, 0.5, rest, this),
        Snap::LeftThird => halves(Direction::Horizontal, THIRD, this, rest),
        Snap::RightThird => halves(Direction::Horizontal, 1.0 - THIRD, rest, this),
        Snap::MiddleThird => {
            let mut ids = Vec::new();
            collect_ids(&rest, &mut ids);
            let (left, right) = ids.split_at(ids.len() / 2);
            match (column(left), column(right)) {
                (Some(left), Some(right)) => halves(
                    Direction::Horizontal,
                    THIRD,
                    left,
                    halves(Direction::Horizontal, 0.5, this, right),
                ),
                _ => halves(Direction::Horizontal, THIRD, rest, this),
            }
        }
    }
}

fn collect_ids(node: &Node, ids: &mut Vec<PaneId>) {
    match node {
        Node::Pane(id) => ids.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, ids);
            collect_ids(second, ids);
        }
    }
}

fn collect_panes(node: &Node, area: Rect, focus: PaneId, result: &mut Vec<PaneRect>) {
    match node {
        Node::Pane(id) => result.push(PaneRect {
            id: *id,
            rect: area,
            focused: *id == focus,
        }),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *direction, *ratio);
            collect_panes(first, a, focus, result);
            collect_panes(second, b, focus, result);
        }
    }
}

fn collect_splits(node: &Node, area: Rect, path: Vec<bool>, result: &mut Vec<SplitBorder>) {
    if let Node::Split {
        direction,
        ratio,
        first,
        second,
    } = node
    {
        let (a, b) = split_rect(area, *direction, *ratio);
        let pos = match direction {
            Direction::Horizontal => a.x.saturating_add(a.width).saturating_sub(1),
            Direction::Vertical => a.y.saturating_add(a.height).saturating_sub(1),
        };
        result.push(SplitBorder {
            pos,
            direction: *direction,
            area,
            path: path.clone(),
        });
        let mut left = path.clone();
        left.push(false);
        collect_splits(first, a, left, result);
        let mut right = path;
        right.push(true);
        collect_splits(second, b, right, result);
    }
}

fn split_at(node: Node, target: PaneId, direction: Direction, new_id: PaneId, ratio: f32) -> Node {
    match node {
        Node::Pane(id) if id == target => Node::Split {
            direction,
            ratio,
            first: Box::new(Node::Pane(id)),
            second: Box::new(Node::Pane(new_id)),
        },
        Node::Pane(_) => node,
        Node::Split {
            direction: d,
            ratio: r,
            first,
            second,
        } => Node::Split {
            direction: d,
            ratio: r,
            first: Box::new(split_at(*first, target, direction, new_id, ratio)),
            second: Box::new(split_at(*second, target, direction, new_id, ratio)),
        },
    }
}

fn remove_pane(node: Node, target: PaneId) -> Option<Node> {
    match node {
        Node::Pane(id) if id == target => None,
        Node::Pane(_) => Some(node),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => match (remove_pane(*first, target), remove_pane(*second, target)) {
            (None, Some(kept)) | (Some(kept), None) => Some(kept),
            (Some(f), Some(s)) => Some(Node::Split {
                direction,
                ratio,
                first: Box::new(f),
                second: Box::new(s),
            }),
            (None, None) => None,
        },
    }
}

fn valid_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.1, 0.9)
    } else {
        0.5
    }
}

fn set_ratio_at(node: &mut Node, path: &[bool], new_ratio: f32) -> bool {
    let Node::Split {
        ratio,
        first,
        second,
        ..
    } = node
    else {
        return false;
    };
    match path.split_first() {
        None => {
            *ratio = new_ratio;
            true
        }
        Some((true, rest)) => set_ratio_at(second, rest, new_ratio),
        Some((false, rest)) => set_ratio_at(first, rest, new_ratio),
    }
}

fn ratio_at(node: &Node, path: &[bool]) -> Option<f32> {
    let Node::Split {
        ratio,
        first,
        second,
        ..
    } = node
    else {
        return None;
    };
    match path.split_first() {
        None => Some(*ratio),
        Some((true, rest)) => ratio_at(second, rest),
        Some((false, rest)) => ratio_at(first, rest),
    }
}

fn split_rect(area: Rect, direction: Direction, ratio: f32) -> (Rect, Rect) {
    // Invariant: children overlap by one border row/column so the raster unions the shared
    // line into junctions; no pane shrinks under 3 cells a side, so tiny terminals hold.
    match direction {
        Direction::Horizontal => {
            let ideal = (f32::from(area.width) * valid_ratio(ratio)).round();
            let first_w = clamp_span(ideal, area.width);
            let second_x = area.x.saturating_add(first_w).saturating_sub(1);
            let second_w = area
                .width
                .saturating_sub(first_w)
                .saturating_add(1)
                .min(area.width);
            (
                Rect::new(area.x, area.y, first_w, area.height),
                Rect::new(second_x, area.y, second_w, area.height),
            )
        }
        Direction::Vertical => {
            let ideal = (f32::from(area.height) * valid_ratio(ratio)).round();
            let first_h = clamp_span(ideal, area.height);
            let second_y = area.y.saturating_add(first_h).saturating_sub(1);
            let second_h = area
                .height
                .saturating_sub(first_h)
                .saturating_add(1)
                .min(area.height);
            (
                Rect::new(area.x, area.y, area.width, first_h),
                Rect::new(area.x, second_y, area.width, second_h),
            )
        }
    }
}

fn clamp_span(ideal: f32, total: u16) -> u16 {
    let min = 3_u16.min(total);
    let max = total.saturating_sub(min).max(min);
    if !ideal.is_finite() || ideal < 0.0 {
        return min;
    }
    // Invariant: bounded is finite, non-negative and <= u16::MAX here.
    let bounded = ideal.min(f32::from(u16::MAX));
    let span = bounded as u16;
    span.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> (TileLayout, PaneIds, [PaneId; 3]) {
        let mut ids = PaneIds::default();
        let (mut layout, a) = TileLayout::new(&mut ids);
        let b = layout.split_focused(&mut ids, Direction::Horizontal);
        let c = layout.split_focused(&mut ids, Direction::Vertical);
        (layout, ids, [a, b, c])
    }

    fn rect(layout: &TileLayout, id: PaneId) -> Rect {
        let panes = layout.panes(Rect::new(0, 0, 90, 30));
        panes
            .iter()
            .find(|pane| pane.id == id)
            .map_or(Rect::default(), |pane| pane.rect)
    }

    #[test]
    fn a_panel_snapped_to_a_third_takes_that_third_of_the_screen() {
        for (snap, x, width) in [
            (Snap::LeftThird, 0, 30),
            (Snap::RightThird, 60, 30),
            (Snap::Left, 0, 45),
            (Snap::Right, 45, 45),
        ] {
            let (mut layout, _, [a, _, c]) = three();
            assert!(layout.snap(c, snap));
            let got = rect(&layout, c);
            assert_eq!(
                (
                    got.x.abs_diff(x) <= 1,
                    got.width.abs_diff(width) <= 1,
                    got.height
                ),
                (true, true, 30),
                "{snap:?}: {got:?}"
            );
            assert_eq!(layout.pane_count(), 3);
            assert_eq!(layout.focused(), c);
            assert!(layout.pane_ids().contains(&a));
        }
    }

    #[test]
    fn the_middle_third_puts_the_others_on_both_sides_of_it() {
        let (mut layout, _, [a, b, c]) = three();
        assert!(layout.snap(a, Snap::MiddleThird));
        let (left, mid, right) = (rect(&layout, b), rect(&layout, a), rect(&layout, c));
        assert!(left.right() <= mid.x.saturating_add(1), "{left:?} {mid:?}");
        assert!(
            mid.right() <= right.x.saturating_add(1),
            "{mid:?} {right:?}"
        );
        assert!(
            mid.width.abs_diff(30) <= 1,
            "the middle is not a third: {mid:?}"
        );
    }

    #[test]
    fn a_lone_panel_has_nowhere_to_snap_and_a_new_one_lands_on_the_edge() {
        let mut ids = PaneIds::default();
        let (mut layout, only) = TileLayout::new(&mut ids);
        assert!(!layout.snap(only, Snap::Left), "a lone panel moved");
        let fresh = layout.snap_new(&mut ids, Snap::Right);
        assert_eq!(layout.pane_count(), 2);
        let got = layout.panes(Rect::new(0, 0, 80, 24));
        let at = |id| {
            got.iter()
                .find(|pane| pane.id == id)
                .map(|pane| pane.rect.x)
        };
        assert!(at(fresh) > at(only), "the new panel is not on the right");
    }

    #[test]
    fn split_close_and_navigate() {
        let mut ids = PaneIds::default();
        let (mut layout, first) = TileLayout::new(&mut ids);
        let area = Rect::new(0, 0, 80, 24);
        let second = layout.split_focused(&mut ids, Direction::Horizontal);
        assert_eq!(layout.pane_count(), 2);
        assert_eq!(layout.focused(), second);
        assert!(layout.focus_direction(NavDirection::Left, area));
        assert_eq!(layout.focused(), first);
        assert!(!layout.focus_direction(NavDirection::Left, area));
        let closed = layout.close_focused();
        assert_eq!(closed, Some(first));
        assert_eq!(layout.focused(), second);
        assert!(layout.close_focused().is_none());
    }

    #[test]
    fn tiny_area_never_overflows() {
        let mut ids = PaneIds::default();
        let (mut layout, _first) = TileLayout::new(&mut ids);
        for _ in 0..4 {
            layout.split_focused(&mut ids, Direction::Horizontal);
            layout.split_focused(&mut ids, Direction::Vertical);
        }
        let area = Rect::new(0, 0, 20, 8);
        let panes = layout.panes(area);
        assert_eq!(panes.len(), 9);
        for pane in panes {
            assert!(pane.rect.right() <= 20 && pane.rect.bottom() <= 8);
        }
    }

    #[test]
    fn ratio_paths_round_trip() -> Result<(), &'static str> {
        let mut ids = PaneIds::default();
        let (mut layout, _first) = TileLayout::new(&mut ids);
        layout.split_focused(&mut ids, Direction::Horizontal);
        let area = Rect::new(0, 0, 80, 24);
        let splits = layout.splits(area);
        let border = splits.first().ok_or("one split exists")?;
        assert!(layout.set_ratio_at(&border.path, 0.7));
        assert_eq!(layout.ratio_at(&border.path), Some(0.7));
        assert!(layout.set_ratio_at(&border.path, 5.0));
        assert_eq!(layout.ratio_at(&border.path), Some(0.9));
        Ok(())
    }
}
