---
version: 0.1105.0
issue: Refs #2308
cut: none desktop/crates/ui/src/menu/context.rs
cut: desktop/crates/app/src/shell/view/scenes.rs | step(&mut hub.model, Msg::Grid(GridMsg::Split(parent))); | a child's chat is a pane about the child that the grid opens; the scene no longer splits the parent's chat and binds the copy by hand
cut: none desktop/crates/app/src/shell/view/tests/panes.rs
cut: desktop/crates/app/src/windows/hub.rs | fn hold_children(hub: &mut Hub) { | the grid file keeps what each pane is about, so the separate kept list of child chat panes is gone
cut: desktop/crates/app/src/windows/places.rs | pub fn keep_children(&mut self, children: &KeptChildren) -> Result<(), PrefsError> { | the grid file keeps what each pane is about, so the separate kept list of child chat panes is gone
cut: none desktop/crates/core/src/command/run.rs
cut: desktop/crates/core/src/grid/step.rs | fn open(grid: &mut Windows, id: WindowId, kind: PaneKind) -> Result<Vec<WindowOp>, GridError> { | opening lives in step/opening.rs and seats a pane at its place; a new pane no longer takes half of the largest pane
cut: none desktop/crates/core/src/grid/step/random.rs
cut: desktop/crates/core/src/grid/step/tests.rs | fn a_pane_of_a_kind_the_window_lacks_opens_in_half_of_the_largest_pane() -> Check { | a new pane joins its place's tabs; the halving rule went, and its test with it
cut: desktop/crates/core/src/grid/swap.rs | Spot::Onto(other) => swap(layout, pane, other)?, | a pane dropped on another's middle joins its tabs; the two no longer swap slots
cut: none desktop/crates/core/src/grid/windows.rs
cut: desktop/crates/panes/src/chat/pane/child.rs | pub struct Children(pub BTreeMap<PaneId, Child>); | the child a chat pane shows is the pane's thing in the grid; the table beside the grid is gone
cut: desktop/crates/panes/src/code_view/pane.rs | fn of(hub: &'a Hub) -> Option<Shown<'a>> { | each code pane shows its own file; the one file every code pane showed is gone
cut: desktop/crates/panes/src/code_view/refs.rs | fn shown(hub: &Hub) -> Option<(&str, Option<&str>)> { | references are asked of the pane's own file, not of the one file every code pane showed
cut: none desktop/crates/ui/src/chrome/tabs.rs
cut: desktop/crates/ui/src/grid_view/view.rs | dispatch(win, Msg::Grid(GridMsg::Split(pane))); | a split goes through beside::split, which puts the viewer's file beside it; a second viewer no longer mirrors the first
cut: desktop/crates/ui/src/shown.rs | if saved != Some(JUST_CHAT.slug) { | a new thread's panes arrive by the rules; its first words no longer pick a layout
cut: none desktop/crates/core/src/grid/step/resting.rs
cut: none desktop/crates/ui/src/overlay/pick.rs
---
In Yi Desktop, point at a file, a line of code, a thread or any other thing and press Space to peek at it: a card over the window shows a file's first lines with their numbers, or the thing's own words, and opens no pane and no tab. Space again, Esc or a click closes it. In a list, with the pointer on nothing, Space peeks the row the arrow keys stand on.
