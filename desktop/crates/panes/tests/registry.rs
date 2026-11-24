//! The pane table is complete: a kind with no definition or no scene cannot be opened or seen.

use std::collections::BTreeSet;

use yd_core::pane::PaneKind;
use yd_panes::registry::{PANES, scenes};
use yd_ui::pane::Stage;

#[test]
fn every_pane_kind_has_a_definition_and_a_scene() {
    for kind in PaneKind::ALL {
        let defs: Vec<_> = PANES.iter().filter(|def| def.kind == *kind).collect();
        assert_eq!(defs.len(), 1, "{kind:?} has {} definitions", defs.len());
        let shown = defs.iter().flat_map(|def| def.scenes).any(|scene| {
            matches!(scene.stage, Stage::Pane(of) if of == *kind)
                && scene.name.starts_with(&format!("{}/", kind.slug()))
        });
        assert!(
            shown,
            "{kind:?} has no scene named {}/<state> that shows it",
            kind.slug()
        );
    }
    assert_eq!(
        PANES.len(),
        PaneKind::ALL.len(),
        "the table has a row for no kind"
    );
}

/// A scene is found by name on the command line and becomes a file path: `<pane>/<state>`.
#[test]
fn every_scene_name_is_one_pane_and_one_state() {
    let mut seen = BTreeSet::new();
    for scene in scenes() {
        let parts: Vec<&str> = scene.name.split('/').collect();
        let plain = |part: &&str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        assert!(
            parts.len() == 2 && parts.iter().all(plain),
            "{:?} is not <pane>/<state>",
            scene.name
        );
        assert!(
            seen.insert(scene.name),
            "two scenes are named {}",
            scene.name
        );
    }
}
