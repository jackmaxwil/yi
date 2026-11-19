//! What a user granting a plugin pane its reach would notice if it broke.

use std::error::Error;

use rune::{Cx, El, ElementInfo, Env, Runtime, col};
use yd_core::ids::WindowId;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::DEF;
use crate::script_pane::pane::library::Inputs;
use crate::script_pane::pane::scenes::queue_depth;

type Check = Result<(), Box<dyn Error>>;

const REACH: &str = "plugin-kit-0.may.reach-ci-ubuntu-on-port-9100";

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

fn drawn() -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    queue_depth(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (316.0, 600.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.016);
    Ok(rt)
}

fn tap(rt: &mut Runtime<Win>, key: &str) -> Check {
    let tree = rt.elements();
    let hit = tree
        .iter()
        .find(|e: &&ElementInfo| e.key.as_deref() == Some(key));
    let [x, y, w, h] = hit.ok_or_else(|| format!("no {key}"))?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    Ok(())
}

fn granted(rt: &Runtime<Win>) -> Result<bool, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    Ok(Inputs::of(&hub.model).granted("queue-depth", "ci-ubuntu:9100"))
}

#[test]
fn the_reach_switch_takes_the_grant_away_and_gives_it_back() -> Check {
    let mut rt = drawn()?;
    assert!(granted(&rt)?, "the scene's grant is on");
    tap(&mut rt, REACH)?;
    assert!(!granted(&rt)?, "pressed, it is off");
    tap(&mut rt, REACH)?;
    assert!(granted(&rt)?, "pressed again, it is on");
    Ok(())
}

#[test]
fn reading_files_says_it_is_not_available_yet() -> Check {
    let mut rt = drawn()?;
    tap(&mut rt, "plugin-kit-0.may.read-your-files")?;
    let tree = rt.elements();
    let said = tree.iter().filter_map(|e| e.text.as_deref());
    assert!(
        said.into_iter()
            .any(|t| t == "Read your files is not available yet")
    );
    Ok(())
}
