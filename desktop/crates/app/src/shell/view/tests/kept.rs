//! The user's own files at start-up and while the window runs, and the settings that show
//! the theme and the layout. Every file here is under the lane's `target/`.

use std::path::{Path, PathBuf};

use yd_core::command::by_kind::{FINDING, SETTING_RESET, items};
use yd_core::command::table::FLIP_CHAT;
use yd_core::findings::triage;
use yd_core::grid::layout::ChatSide;
use yd_core::ids::{SettingKey, ThemeId};
use yd_core::pane::PaneKind;
use yd_core::settings::schema::SettingValue;
use yd_core::tokens::Rgba;
use yd_panes::settings::pane::act;
use yd_panes::settings::pane::state::{State, Tab};
use yd_panes::themes::pane::{DENSITY, THEME};
use yd_ui::grid_view::picker::{CHAT_SIDE_KEY as CHAT_SIDE, LAYOUT_KEY as LAYOUT};
use yd_ui::menu::command_box;
use yd_ui::theme::files::{self, Library};
use yd_ui::win::Hub;

use super::*;
use crate::start::{self, Files};

const SEA: &str =
    r##"{"id": "sea", "name": "Sea", "dark": true, "values": {"accent": "#7fc4e8"}}"##;
const MADE: &str = r#"{"decisions": [], "next": 1, "made": [{"id": "made/0",
    "evidence": "yi://file/src/lib.rs", "title": "lib.rs", "at": 5}]}"#;

/// An empty folder of this test's own under the lane's target.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/in6-kept")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("themes"))?;
    Ok(dir)
}

/// The window as the native app starts it over the files of `folder`.
fn started(folder: &Path) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    start::files(&mut *win.hub.try_borrow_mut()?, Files::under(folder));
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    Ok(runtime)
}

fn with<T>(runtime: &Runtime<Win>, read: impl FnOnce(&Hub) -> T) -> Result<T, Box<dyn Error>> {
    Ok(read(&*runtime.state.hub.try_borrow()?))
}

fn theme_in_use(runtime: &Runtime<Win>) -> Result<(String, Rgba), Box<dyn Error>> {
    with(runtime, |hub| {
        let active = hub.panes.get::<Library>(None).map(|l| l.active.to_string());
        (active.unwrap_or_default(), hub.model.tokens.accent)
    })
}

fn setting(runtime: &Runtime<Win>, key: &str) -> Result<Option<SettingValue>, Box<dyn Error>> {
    with(runtime, |hub| hub.model.settings.value(key).cloned())
}

fn text(value: &str) -> Option<SettingValue> {
    Some(SettingValue::Text(value.to_owned().into()))
}

#[test]
fn start_up_reads_the_users_settings_themes_and_triage() -> Check {
    let dir = folder("reads")?;
    std::fs::write(dir.join("themes/sea.json"), SEA)?;
    std::fs::write(dir.join("settings.json"), r#"{"appearance.theme": "sea"}"#)?;
    std::fs::write(dir.join("findings.json"), MADE)?;
    let runtime = started(&dir)?;
    let sea = Rgba::parse("#7fc4e8").ok_or("no colour")?;
    assert_eq!(theme_in_use(&runtime)?, ("sea".to_owned(), sea));
    let made = with(&runtime, |hub| hub.model.findings.triage.made.len())?;
    assert_eq!(made, 1, "the triage file was read");
    Ok(())
}

#[test]
fn a_theme_put_in_use_is_the_theme_after_a_restart() -> Check {
    let dir = folder("restart")?;
    let mut runtime = started(&dir)?;
    runtime.update(|win, tx| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(library) = files::library(&mut hub)
        {
            library.pick(&ThemeId::new("ink"));
        }
        files::apply(win, Some(tx));
    });
    rest(&mut runtime);
    assert_eq!(setting(&runtime, THEME)?, text("ink"), "settings show it");
    start::quit(&mut runtime.state);
    let again = started(&dir)?;
    let ink = Rgba::parse("#a79cf0").ok_or("no colour")?;
    assert_eq!(theme_in_use(&again)?, ("ink".to_owned(), ink));
    Ok(())
}

#[test]
fn a_finding_made_in_the_window_is_there_after_a_restart() -> Check {
    let dir = folder("triage")?;
    let mut runtime = started(&dir)?;
    let file = Target::Thing(Address::new(Kind::File, "src/lib.rs")?);
    runtime.update(move |win, tx| {
        command_box::run(win, tx, Run::new(FINDING).on(file.clone()));
    });
    rest(&mut runtime);
    start::quit(&mut runtime.state);
    let kept = triage::load(&dir.join("findings.json"));
    assert_eq!(kept.made.len(), 1, "the finding reached the file");
    let again = started(&dir)?;
    let made = with(&again, |hub| hub.model.findings.triage.made.len())?;
    assert_eq!(made, 1);
    Ok(())
}

/// A triage file that did not read and could not be moved aside stays as it is.
#[test]
fn a_triage_file_that_did_not_read_is_never_written_over() -> Check {
    let dir = folder("unread")?;
    let path = dir.join("findings.json");
    std::fs::write(&path, "{ not triage")?;
    // The move aside cannot happen: a folder that is not empty has that name.
    std::fs::create_dir_all(dir.join("findings.json.bad/held"))?;
    let mut runtime = started(&dir)?;
    let file = Target::Thing(Address::new(Kind::File, "src/lib.rs")?);
    runtime.update(move |win, tx| {
        command_box::run(win, tx, Run::new(FINDING).on(file.clone()));
    });
    rest(&mut runtime);
    start::quit(&mut runtime.state);
    assert_eq!(std::fs::read_to_string(&path)?, "{ not triage");
    Ok(())
}

#[test]
fn an_idle_window_notices_a_theme_file_changed_on_disk() -> Check {
    let dir = folder("watch")?;
    let file = dir.join("themes/sea.json");
    std::fs::write(&file, SEA)?;
    std::fs::write(dir.join("settings.json"), r#"{"appearance.theme": "sea"}"#)?;
    let mut runtime = started(&dir)?;
    std::fs::write(
        &file,
        SEA.replace("#7fc4e8", "#112233").replace("Sea", "Deep sea"),
    )?;
    for _ in 0..4 {
        runtime.frame(0.5);
    }
    let deep = Rgba::parse("#112233").ok_or("no colour")?;
    assert_eq!(theme_in_use(&runtime)?, ("sea".to_owned(), deep));
    Ok(())
}

#[test]
fn settings_list_the_theme_and_layout_rows_the_app_declares() -> Check {
    let win = root::init();
    let hub = win.hub.try_borrow()?;
    let keys: Vec<&str> = hub
        .model
        .settings
        .rows
        .iter()
        .map(|row| row.key.as_str())
        .collect();
    for key in [THEME, DENSITY, "appearance.glass", CHAT_SIDE, LAYOUT] {
        assert!(keys.contains(&key), "{key} is not listed: {keys:?}");
    }
    Ok(())
}

/// The pane of `kind` the main window draws.
fn pane_of(runtime: &Runtime<Win>, kind: PaneKind) -> Result<Option<u32>, Box<dyn Error>> {
    with(runtime, |hub| {
        let grid = &hub.model.grid;
        let docked = grid.window(root::MAIN)?.layout.docked();
        let mut panes = docked.map(|(pane, _)| pane);
        panes
            .find(|pane| grid.kind(*pane) == Some(kind))
            .map(|pane| pane.0)
    })
}

#[test]
fn the_edit_chip_on_the_theme_row_opens_the_themes_pane() -> Check {
    let mut runtime = unframed("shell/everything")?;
    // On a wide screen, beside the chat alone, the pane has room for its three columns.
    runtime.set_viewport((2000.0, 900.0), 1.0);
    runtime.frame(1.0 / 60.0);
    runtime.update(|win, tx| {
        yd_ui::grid_view::view::act(win, tx, yd_ui::grid_view::view::Act::Show("just-chat"));
        yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Settings);
    });
    let settings = pane_of(&runtime, PaneKind::Settings)?.ok_or("no settings pane")?;
    runtime.update(move |win, _| {
        let pane = yd_core::ids::PaneId(settings);
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
        {
            state.tab = Tab::All;
        }
    });
    rest(&mut runtime);
    assert_eq!(pane_of(&runtime, PaneKind::Themes)?, None);
    let shown = runtime.elements();
    let edit = keys(&shown)
        .into_iter()
        .find(|key| key.contains(THEME) && key.ends_with(".edit"))
        .ok_or("no Edit chip on the theme row")?
        .to_owned();
    tap(&mut runtime, &edit)?;
    assert_eq!(pane_of(&runtime, PaneKind::Themes)?, None, "not docked");
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let themes = grid.panes.iter().find(|pane| pane.kind == PaneKind::Themes);
    let themes = themes.ok_or("the Themes pane was not made")?;
    assert!(
        grid.origin_of(themes.id).is_some(),
        "Themes is shown in a window of its own"
    );
    Ok(())
}

#[test]
fn the_chat_side_setting_and_the_grid_follow_each_other() -> Check {
    let mut runtime = runtime("shell/everything")?;
    runtime.update(|win, tx| {
        command_box::run(win, tx, Run::new(FLIP_CHAT));
    });
    rest(&mut runtime);
    assert_eq!(
        setting(&runtime, CHAT_SIDE)?,
        text("right"),
        "the flip shows"
    );
    let key = SettingKey::new(CHAT_SIDE);
    runtime.update(move |win, _| act::set(win, yd_ui::pane::SCENE_PANE, &key, text("left")));
    rest(&mut runtime);
    let side = with(&runtime, |hub| hub.model.grid.side)?;
    assert_eq!(side, ChatSide::Left, "the setting moved the chat");
    Ok(())
}

#[test]
fn the_density_setting_sets_the_sizes_of_the_theme_in_use() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let key = SettingKey::new(DENSITY);
    runtime.update(move |win, _| act::set(win, yd_ui::pane::SCENE_PANE, &key, text("roomy")));
    rest(&mut runtime);
    let sizes = with(&runtime, |hub| {
        let tokens = &hub.model.tokens;
        (tokens.row, tokens.line, tokens.gutter)
    })?;
    assert_eq!(sizes, (24.0, 18.0, 10.0));
    assert_eq!(setting(&runtime, DENSITY)?, text("roomy"));
    Ok(())
}

#[test]
fn a_settings_right_click_rows_copy_its_key_and_put_its_default_back() -> Check {
    let labels: Vec<&str> = items(Kind::Setting).iter().map(|item| item.label).collect();
    assert_eq!(labels, ["Copy its key", "Put the default back"]);
    let mut runtime = runtime("shell/everything")?;
    runtime.update(|win, tx| {
        command_box::run(win, tx, Run::new(FLIP_CHAT));
    });
    rest(&mut runtime);
    let row = Target::Thing(Address::new(Kind::Setting, CHAT_SIDE)?);
    runtime.update(move |win, tx| {
        command_box::run(win, tx, Run::new(SETTING_RESET).on(row.clone()));
    });
    rest(&mut runtime);
    assert_eq!(setting(&runtime, CHAT_SIDE)?, text("left"));
    let side = with(&runtime, |hub| hub.model.grid.side)?;
    assert_eq!(side, ChatSide::Left, "and the chat is back on its side");
    Ok(())
}
