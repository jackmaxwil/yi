//! The scenes of the engines in this crate. An engine adds a scene in its own file; the
//! tables here already point at it.

use crate::pane::Scene;
use crate::{chrome, code, grid_view, kit, list, menu, overlay, screen, states, term};

pub const TABLES: &[&[Scene]] = &[
    kit::gallery::SCENES,
    chrome::tabs::SCENES,
    grid_view::view::SCENES,
    list::view::SCENES,
    code::view::SCENES,
    term::view::SCENES,
    screen::surface::SCENES,
    menu::command_box::SCENES,
    menu::context::SCENES,
    overlay::boxes::SCENES,
    states::status::SCENES,
];
