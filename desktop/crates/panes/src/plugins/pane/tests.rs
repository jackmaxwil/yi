//! What a user managing plugin panes would notice if it broke.

use std::error::Error;
use std::path::{Path, PathBuf};

use rune::{Cx, El, Env, Runtime, col};
use yd_core::address::uri::Address;
use yd_core::command::table::Target;
use yd_core::ids::{RowId, WindowId};
use yd_core::kind::Kind;
use yd_core::row::Value;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, carry};
use crate::plugins::desc::{RELOAD, REMOVE};
use crate::script_pane::pane::library::FEED;
use crate::script_pane::pane::scenes::{MANIFEST, NAME, SOURCE};
use crate::script_pane::pane::{library, open, tick};

type Check = Result<(), Box<dyn Error>>;

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

/// A panes folder with Queue depth in it and a Trash beside it, under the lane's `target/`.
fn disk(name: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let at = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/pl")
        .join(format!("{name}-{}", std::process::id()));
    if at.exists() {
        std::fs::remove_dir_all(&at)?;
    }
    let (panes, trash) = (at.join("panes"), at.join("Trash"));
    std::fs::create_dir_all(panes.join(NAME))?;
    std::fs::create_dir_all(&trash)?;
    std::fs::write(panes.join(NAME).join("plugin.json"), MANIFEST)?;
    std::fs::write(panes.join(NAME).join("pane.luau"), SOURCE)?;
    Ok((panes, trash))
}

fn started(panes: &Path, trash: &Path) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        open(&mut hub, panes.to_path_buf());
        if let Some(library) = library(&mut hub) {
            library.trash = Some(trash.to_path_buf());
        }
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (334.0, 700.0),
        Env::new(true, 1.0),
    );
    rt.update(|win, tx| {
        tick(win, 0.0, tx);
    });
    Ok(rt)
}

fn press(rt: &mut Runtime<Win>, command: yd_core::ids::CommandId) -> Result<bool, Box<dyn Error>> {
    let target = Target::Thing(Address::new(Kind::Plugin, NAME)?);
    let mut done = false;
    rt.update(|win, tx| done = carry(win, tx, command, &target));
    Ok(done)
}

fn row(rt: &Runtime<Win>) -> Result<Option<yd_core::row::Row>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    Ok(hub
        .model
        .tables
        .get(FEED)
        .and_then(|t| t.find(&RowId::new(NAME)).cloned()))
}

#[test]
fn remove_asks_first_then_moves_the_folder_to_the_trash() -> Check {
    let (panes, trash) = disk("remove")?;
    let mut rt = started(&panes, &trash)?;
    assert!(row(&rt)?.is_some(), "the plugin on disk is listed");
    assert!(press(&mut rt, REMOVE)?);
    assert!(
        panes.join(NAME).join("pane.luau").is_file(),
        "one press only asks"
    );
    let asked = row(&rt)?.ok_or("no row")?;
    assert_eq!(asked.get("confirm"), Some(&Value::Flag(true)));
    assert!(press(&mut rt, REMOVE)?);
    assert!(
        !panes.join(NAME).exists(),
        "the folder left the panes folder"
    );
    let kept = std::fs::read_to_string(trash.join(NAME).join("pane.luau"))?;
    assert_eq!(kept, SOURCE, "moved whole, nothing deleted");
    assert!(row(&rt)?.is_none(), "and it is no longer listed");
    Ok(())
}

#[test]
fn reload_reads_the_files_again_and_moves_the_version() -> Check {
    let (panes, trash) = disk("reload")?;
    let mut rt = started(&panes, &trash)?;
    assert_eq!(
        row(&rt)?.ok_or("no row")?.words("version"),
        Some("version 1")
    );
    assert!(press(&mut rt, RELOAD)?);
    assert_eq!(
        row(&rt)?.ok_or("no row")?.words("version"),
        Some("version 2")
    );
    Ok(())
}
