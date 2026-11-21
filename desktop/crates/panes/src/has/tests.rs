//! Each kind the saved layouts seat has nothing to show in a session with nothing in it, and
//! has once the scene that builds its mockup state has run.

use std::error::Error;

use yd_core::ids::WindowId;
use yd_core::pane::PaneKind;
use yd_ui::win::Hub;

use super::kinds;
use crate::registry::{PANES, scenes};

const MAIN: WindowId = WindowId(0);

/// The kinds that rest, each with the scene whose fixtures give it something to show.
const BUILT: [(PaneKind, &str); 8] = [
    (PaneKind::Files, "files/mockup"),
    (PaneKind::Changes, "changes/mockup"),
    (PaneKind::Terminal, "terminal/mockup"),
    (PaneKind::Code, "code/mockup"),
    (PaneKind::Web, "web/mockup"),
    (PaneKind::Remote, "desktop/mockup"),
    (PaneKind::Report, "report/mockup"),
    (PaneKind::Agents, "agents/mockup"),
];

#[test]
fn a_kind_has_something_to_show_only_once_its_fixtures_are_in() -> Result<(), Box<dyn Error>> {
    for (kind, name) in BUILT {
        let hub = Hub::new(PANES, scenes().collect());
        let mut hub = hub.try_borrow_mut()?;
        assert!(!kinds(&hub, MAIN).contains(&kind), "{kind:?} with nothing");
        assert!(
            kinds(&hub, MAIN).contains(&PaneKind::Chat),
            "chat always has"
        );
        let scene = scenes().find(|scene| scene.name == name);
        (scene.ok_or(name)?.setup)(&mut hub);
        assert!(kinds(&hub, MAIN).contains(&kind), "{kind:?} after {name}");
    }
    Ok(())
}
