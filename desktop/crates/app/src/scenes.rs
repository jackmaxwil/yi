//! The scene table: every scene of every crate, in one order. Scene `I` runs in slot `I`.

use yd_ui::pane::Scene;

use crate::{shell, windows};

/// Scenes of the app itself.
const APP: &[&[Scene]] = &[shell::view::SCENES, windows::hub::SCENES];

pub fn all() -> Vec<&'static Scene> {
    let tables = yd_ui::scenes::TABLES.iter().chain(APP);
    tables
        .copied()
        .flatten()
        .chain(yd_panes::registry::scenes())
        .collect()
}
