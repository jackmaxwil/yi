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
const BUILT: [(PaneKind, &str); 9] = [
    (PaneKind::Files, "files/mockup"),
    (PaneKind::Changes, "changes/mockup"),
    (PaneKind::Terminal, "terminal/mockup"),
    (PaneKind::Code, "code/mockup"),
    (PaneKind::Web, "web/mockup"),
    (PaneKind::Remote, "desktop/mockup"),
    (PaneKind::Report, "report/mockup"),
    (PaneKind::Agents, "agents/mockup"),
    (PaneKind::IPython, "ipython/picture"),
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

/// A kind the person always opens has its pane to show with nothing in it; a word of never
/// gives a kind nothing.
#[test]
fn a_kind_always_opened_has_its_pane_to_show_with_nothing_in_it() -> Result<(), Box<dyn Error>> {
    use yd_core::settings::schema::SettingValue;
    use yd_core::settings::store::OPENS_ROW;
    let hub = Hub::new(PANES, scenes().collect());
    let mut hub = hub.try_borrow_mut()?;
    let mut row = OPENS_ROW;
    row.value = Some(SettingValue::Text("changes work\nweb never".into()));
    hub.model.settings.rows.push(row);
    let now = kinds(&hub, MAIN);
    assert!(now.contains(&PaneKind::Changes), "always: {now:?}");
    assert!(!now.contains(&PaneKind::Web), "never has nothing: {now:?}");
    Ok(())
}

/// Kernel cells that print and return values are in the chat already: the recorded turn
/// gives IPython nothing to show until its fourth cell draws a chart.
#[test]
fn cells_with_no_picture_give_ipython_nothing_to_show() -> Result<(), Box<dyn Error>> {
    let hub = Hub::new(PANES, scenes().collect());
    let mut hub = hub.try_borrow_mut()?;
    let scene = scenes().find(|scene| scene.name == "ipython/running");
    (scene.ok_or("no running scene")?.setup)(&mut hub);
    let thread = yd_ui::shown::thread(&hub, MAIN).ok_or("no thread shown")?;
    assert!(thread.transcript.cells().count() >= 2, "it has cells");
    assert!(!kinds(&hub, MAIN).contains(&PaneKind::IPython));
    Ok(())
}
