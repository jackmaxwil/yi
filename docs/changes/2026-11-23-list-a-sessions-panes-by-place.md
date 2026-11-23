---
issue: Refs #2308
cut: desktop/crates/ui/src/grid_view/picker/panel.rs | fn pane_item(hub: &Hub, window: WindowId, kind: PaneKind) -> El { | the Panes panel lists the session's panes by place; the switch for every kind of pane, most of which opened blank, is gone
cut: desktop/crates/app/src/shell/view/tests/panes.rs | fn a_pane_switched_on_is_placed_as_show_places_it_and_off_closes_with_no_hole() -> Check { | the switches are gone; the list that replaced them is tested in tests/places.rs
cut: none desktop/docs/architecture.md
cut: none desktop/docs/layout.md
---
In Yi Desktop, the Panes panel (⌥⌘0) now lists what the session has open, grouped by where it sits: chat, work, side and shelf. Click a row to go to that pane, including a tab hidden behind another or a pane pulled out into its own window; the small cross on a row closes it. Kinds of pane that have something to show but are not open are offered at the bottom under "Not open". The old switch for every kind of pane, most of which opened an empty pane, is gone.
