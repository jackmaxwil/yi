//! The root of every window: what it draws for the stage it is on, and how the app starts.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

use rune::{App, AppSpec, Cx, El, Key, KeyEvent, SystemMaterial, Titlebar, Tx, WindowOptions, col};
use yd_core::command::table::{ACTUAL_SIZE, ZOOM_IN, ZOOM_OUT};
use yd_core::grid::saved::BLANK;
use yd_core::grid::step::GridMsg;
use yd_core::ids::{CommandId, SettingKey, WindowId};
use yd_core::msg::{Msg, step};
use yd_core::settings::schema::{Control, Setting, SettingValue, Store};
use yd_core::settings::store::SettingsMsg;
use yd_core::tokens::Tokens;
use yd_panes::new_chat::view as new_chat;
use yd_panes::settings::pane::file;
use yd_ui::grid_view::view::alone;
use yd_ui::menu::{command_box, context};
use yd_ui::overlay::boxes;
use yd_ui::pane::Stage;
use yd_ui::slots::SCENE_SLOTS;
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win, drain};

use crate::{notify, scenes, seen, shell, start, windows};

/// A scene as Rune takes it: its name and the function that puts a window in it.
type NamedScene = (&'static str, fn(&mut Win));

/// The window a single-window run has.
pub const MAIN: WindowId = WindowId(0);
/// The layout a window opens in until a saved one is read.
pub const FIRST_LAYOUT: &str = "everything";
/// The band the traffic lights sit in: the top bar's own height.
pub const TITLEBAR: f32 = shell::top_bar::HEIGHT;
/// The faces every window names: plain text and code.
pub const FAMILIES: (&str, &str) = ("System Font", "ui-monospace");

/// The steps ⌘= and ⌘- move the zoom through, as a browser's page zoom does.
const ZOOMS: [f64; 12] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5,
];
pub const ZOOM_KEY: &str = "view.zoom";
/// Declared by the native app alone, so a scene or a test draws at the mockup's own size.
/// The owner's own: 1.5 made a 2056 by 1204 window read as the mockup does, too large.
pub const ZOOM: Setting = Setting {
    key: SettingKey::fixed(ZOOM_KEY),
    group: Cow::Borrowed("Appearance"),
    name: Cow::Borrowed("Zoom"),
    description: Cow::Borrowed("How large the window draws everything: ⌘= and ⌘- step it."),
    control: Control::Slider {
        min: 0.5,
        max: 2.5,
        unit: Cow::Borrowed("×"),
    },
    store: Store::App,
    default: Some(SettingValue::Number(1.25)),
    value: None,
};

pub fn init() -> Win {
    let hub = Hub::new(yd_panes::registry::PANES, scenes::all());
    if let Ok(mut hub) = hub.try_borrow_mut() {
        let declared = yd_panes::registry::settings().cloned();
        hub.model.settings.rows.extend(declared);
        let layout = yd_ui::grid_view::picker::SETTINGS.iter().cloned();
        hub.model.settings.rows.extend(layout);
        hub.model.settings.rows.push(start::SOCKET);
        hub.model
            .settings
            .rows
            .push(yd_core::settings::store::PROJECTS_ROW);
        let here = std::env::current_dir()
            .ok()
            .and_then(|dir| dir.to_str().map(str::to_owned));
        if let Some(started) = hub.panes.get_mut::<new_chat::StartedIn>(None) {
            started.0 = here.filter(|dir| dir != "/");
        }
        command_box::install(&mut hub, shell::view::WINDOWED, shell::view::carry);
        // The grid draws nothing for a window it does not hold.
        let show = GridMsg::Show {
            window: MAIN,
            saved: FIRST_LAYOUT,
        };
        step(&mut hub.model, Msg::Grid(show));
        // Incident: cut once as a launch opening blank panes, which the native start never shows;
        // scenes, tests and journeys start here, each pane asked for so none rests with nothing to show.
        let kinds: Vec<_> = hub.model.grid.panes.iter().map(|pane| pane.kind).collect();
        for kind in kinds {
            let window = MAIN;
            step(&mut hub.model, Msg::Grid(GridMsg::Open { window, kind }));
        }
    }
    Win::new(hub, MAIN)
}

/// The material the window's root carries: glass while the `glass` token is on. Off, the
/// root draws its own fill, as every headless picture does.
pub fn material(tokens: &Tokens) -> Option<SystemMaterial> {
    tokens.glass.then_some(SystemMaterial::Glass)
}

pub fn view(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    zoomed(cx, &hub, TITLEBAR);
    let body = match hub.stage {
        Stage::App => shell::view::view(win, &hub, cx),
        Stage::View(view) => view(win, &hub, cx),
        Stage::Pane(kind) => alone(win, &hub, cx, kind),
    };
    dressed(win, &hub, cx, "root", body)
}

/// A window's root, keyed `key`, round what it draws: the boxes round what is attached, the
/// window's menus over everything, paste and right-click, and the window's one material.
pub fn dressed(win: &Win, hub: &Hub, cx: &mut Cx<Win>, key: &'static str, body: El) -> El {
    yd_ui::a11y::focus(cx, &hub.model.tokens);
    let body = boxes::over(body, hub, cx);
    let body = rune_kit::root(body, command_box::layer(win, hub, cx));
    // Invariant: one material a window, on the root, whose own fill is what an opaque
    // window and every headless picture show; the shell's tint is a child.
    let root = col()
        .key(key)
        .w_full()
        .h_full()
        .fill(colour(hub.model.tokens.window))
        .preserve_focus_on_press(true)
        .on_paste(paste)
        .on_drop_files(dropped)
        .accepts_files(Path::is_dir)
        .on_secondary_press_local(context::press)
        .child(body);
    match material(&hub.model.tokens) {
        Some(glass) => root.system_material(glass),
        None => root,
    }
}

/// The window's zoom: the setting, else the Mac's own points.
pub fn zoom(hub: &Hub) -> f64 {
    match hub.model.settings.value(ZOOM_KEY) {
        Some(SettingValue::Number(k)) => *k,
        _ => 1.0,
    }
}

/// Zooms the window, its title band grown with it so the lights stay in the top bar.
pub fn zoomed(cx: &mut Cx<Win>, hub: &Hub, band: f32) {
    // A zoom is a step of the ladder or the user's own number, far inside an `f32`.
    let k = zoom(hub) as f32;
    cx.zoom(k);
    cx.titlebar_height(band * k);
}

/// Carries ⌘=, ⌘- and ⌘0: the next step either way, or actual size, kept in the settings file.
pub fn zoom_to(win: &mut Win, command: CommandId) -> bool {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let now = zoom(&hub);
    let next = match command {
        ZOOM_IN => ZOOMS.into_iter().find(|k| *k > now + 0.01),
        ZOOM_OUT => ZOOMS.into_iter().rev().find(|k| *k < now - 0.01),
        ACTUAL_SIZE => Some(1.0),
        _ => return false,
    };
    let key = SettingKey::fixed(ZOOM_KEY);
    if let Some(next) = next {
        let set = SettingsMsg::Set(key.clone(), Some(SettingValue::Number(next)));
        step(&mut hub.model, Msg::Settings(set));
    }
    if let Err(why) = file::keep(&mut hub, &key) {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: the zoom was not kept: {why}"
        );
    }
    true
}

/// Shows the chat alone in `window`: how a first launch, with nothing kept, opens.
pub fn blank(hub: &mut Hub, window: WindowId) {
    let saved = BLANK.slug;
    step(&mut hub.model, Msg::Grid(GridMsg::Show { window, saved }));
}

/// Steps what the workers sent, opens on the wire and on disk whatever thread this window
/// now shows, and keeps the user's files and the settings that show state in step.
pub fn poll(win: &mut Win, seconds: f64, tx: &mut Tx) -> bool {
    let windows = windows::hub::poll(win, tx);
    let settled = start::settle(win);
    // Before the drain, so a poll it holds asks for no folder that is gone.
    new_chat::forget_gone(win, seconds);
    let drained = drain(win);
    let shown = yd_ui::shown::sync(win);
    let rested = yd_panes::has::tell(win, tx);
    let told = notify::poll(win);
    let hover = yd_ui::code::state::tick(win, tx);
    let followed = yd_panes::lsp::marks::follow(win, tx);
    // Invariant: one window looks at the themes folder and asks for the next look's wake;
    // the main one, which polls even while it is closed.
    let themed = win.window == MAIN && yd_ui::theme::files::tick(win, seconds, tx);
    // The same window runs the plugins: their folder is looked at and their timers kept once.
    let scripted = win.window == MAIN && yd_panes::script_pane::pane::tick(win, seconds, tx);
    let asked = win.window == MAIN && windows::single::answer(win);
    if scripted {
        yd_ui::win::wake_others(&win.hub, win.window);
    }
    let bound = yd_panes::settings::pane::bind::follow(win);
    seen::poll(win);
    start::keep(win);
    let heard = windows || settled || drained || shown || rested || told || hover || followed;
    heard || themed || scripted || bound || asked
}

/// A folder dropped on a window shows New chat on it.
fn dropped(win: &mut Win, paths: &[PathBuf], tx: &mut Tx) {
    if let Some(folder) = paths.first().and_then(|path| path.to_str()) {
        new_chat::pick(win, tx, folder.to_owned());
    }
}

/// Rune hands a paste only to the root; the terminal with the keyboard takes it.
fn paste(win: &mut Win, text: &str, tx: &mut Tx) {
    yd_ui::term::view::paste(win, text, tx);
}

/// Keys nothing focused took: Esc closes the bell's panel, else leaves New chat; then the
/// command box's matcher closes a menu or runs the command the shortcut map gives the key.
pub fn on_key(win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    let leaves = event.key == Key::Escape
        && (yd_panes::notifications::view::key(win, event) || yd_ui::shown::leave(win));
    // ⌘+ is ⌘= with Shift on most layouts: the map holds ⌘=, and both zoom in.
    let plus = event.mods.cmd && matches!(&event.key, Key::Text(typed) if typed == "+");
    leaves || (plus && zoom_to(win, ZOOM_IN)) || command_box::keys(win, event, tx)
}

/// The window as `spikes/glass` proved it: titled, see-through, its content under the title
/// bar with the traffic lights over the top bar.
pub fn app() -> App<Win> {
    let names = scenes::all();
    let table: Vec<NamedScene> = names
        .iter()
        .zip(SCENE_SLOTS)
        .map(|(scene, slot)| (scene.name, slot))
        .collect();
    // Rune keeps the scene table for the life of the process: leaked once, at start-up.
    let scenes: &'static [NamedScene] = Box::leak(table.into_boxed_slice());
    let spec = AppSpec {
        scenes,
        min_size: Some((720.0, 480.0)),
        families: FAMILIES,
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Yi Desktop", (1200.0, 750.0), init, view)
    };
    App::new(spec)
        .titlebar(Titlebar::new(TITLEBAR))
        .window_options(WindowOptions {
            transparent: true,
            ..WindowOptions::default()
        })
}

/// The main window's app: the Mac's menus and notices on it before it runs, and the one
/// ready hook, which starts the live work once the window has drawn.
pub fn main(native: bool, hub: &Hub) -> App<Win> {
    let app = notify::install(app(), hub);
    let handle = app.handle();
    app.on_ready(move |window| {
        let (window, app) = (window.clone(), handle.clone());
        handle.update(move |win, _| {
            start::start(win, app.clone(), native);
            // Invariant: status items and the dock belong to the native app alone.
            if native {
                notify::ready(win, window, &app);
            }
        });
    })
}

/// The one quit hook, in every window still open: the window layer keeps its places, then
/// every waiting ask is answered cancelled and the client and sources stop.
pub fn quit(win: &mut Win, tx: &mut Tx) {
    windows::hub::quit(win, tx);
    start::quit(win);
}

/// Opens the app's windows over one hub, or renders headless for Rune's tooling flags.
/// `native` is false for a headless run: it reaches a server only when told its socket.
pub fn run(native: bool) {
    // A second copy in this home brings the first forward and goes: one app, one status item.
    let folder = rune::dirs::AppDirs::new("yi-desktop").map(|dirs| dirs.config().to_path_buf());
    let single = match folder
        .filter(|_| native)
        .map(|folder| windows::single::hold(&folder))
    {
        Some(Ok(None)) => return,
        Some(Ok(Some(single))) => Some(single),
        Some(Err(_)) | None => None,
    };
    yd_sources::browser::hold_pipe_fds();
    let shared = init().hub;
    if let (Some(single), Ok(mut hub)) = (single, shared.try_borrow_mut())
        && let Some(kept) = hub.panes.get_mut::<windows::single::Single>(None)
    {
        *kept = single;
    }
    // Invariant: panes lay out and ask before the ready hook installs the sink.
    if start::connects(native)
        && let Ok(mut hub) = shared.try_borrow_mut()
    {
        hub.hold();
    }
    // Invariant: only the native app reads the user's own files; a scene or a test is
    // given a folder of its own, or none.
    if native && let (Ok(mut hub), Some(files)) = (shared.try_borrow_mut(), start::Files::app()) {
        // The Keys rows go in before the file is read, so a key the user set is found; a
        // scene's settings list would otherwise start with seventy of them.
        let keys = yd_core::command::keys::settings(&command_box::table(&hub));
        hub.model.settings.rows.extend(keys);
        hub.model.settings.rows.push(ZOOM);
        start::files(&mut hub, files);
    }
    let launch = if native {
        windows::hub::Launch::native()
    } else {
        windows::hub::Launch::headless()
    };
    if native
        && launch.places.read(MAIN, &[]).is_none()
        && let Ok(mut hub) = shared.try_borrow_mut()
    {
        blank(&mut hub, MAIN);
    }
    rune::run_windows(move |displays| windows::hub::specs(&shared, displays, launch));
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Env, Runtime};

    use super::*;

    #[test]
    fn the_root_carries_glass_only_while_the_token_is_on() {
        let mut tokens = Tokens::default();
        assert_eq!(material(&tokens), Some(SystemMaterial::Glass));
        tokens.glass = false;
        assert_eq!(material(&tokens), None, "off, the root's own fill shows");
    }

    #[test]
    fn the_title_band_is_the_top_bar_and_the_root_keeps_its_opaque_fill()
    -> Result<(), Box<dyn Error>> {
        let mut runtime = Runtime::new(init(), view, (1200.0, 750.0), Env::new(true, 1.0));
        runtime.frame(1.0 / 60.0);
        let shown = runtime.elements();
        let keyed = |key: &str| shown.iter().find(|e| e.key.as_deref() == Some(key));
        let bar = keyed("shell.top-bar").ok_or("no top bar")?.rect;
        assert_eq!(
            (bar[1], bar[3]),
            (0.0, TITLEBAR),
            "the lights sit in the top bar"
        );
        let fill = keyed("root").ok_or("no root")?.fill;
        let window = colour(Tokens::default().window);
        assert_eq!(fill, window, "what an opaque or headless window shows");
        assert_eq!(fill.a, 1.0);
        // Holding the top bar moves the window: the title bar is the app's own.
        runtime.pointer_move(400.0, 12.0);
        runtime.pointer_down(400.0, 12.0);
        runtime.frame(1.0 / 60.0);
        assert_eq!(
            runtime.take_titlebar_action(),
            Some(rune::TitlebarAction::Drag)
        );
        Ok(())
    }

    #[test]
    fn a_shell_laid_out_before_the_app_starts_still_starts() -> Result<(), Box<dyn Error>> {
        let win = init();
        win.hub.try_borrow_mut()?.hold();
        let mut runtime = Runtime::new(win, view, (1200.0, 750.0), Env::new(true, 1.0));
        runtime.poll = Some(poll);
        for _ in 0..30 {
            runtime.frame(1.0 / 60.0);
        }
        // The ready hook installs the sink only once the first frames have drawn.
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = std::rc::Rc::clone(&asked);
        let mut hub = runtime.state.hub.try_borrow_mut()?;
        hub.set_inbox(yd_core::inbox::inbox(|| {}).1);
        hub.set_sink(move |effect| {
            if let yd_core::msg::Effect::Source(feed, ask) = effect {
                log.borrow_mut().push((feed, ask));
            }
        });
        drop(hub);
        runtime.update(|_, _| {});
        runtime.frame(1.0 / 60.0);
        let sized = asked.borrow().iter().any(|(feed, ask)| {
            feed.source == yd_core::feed::TERMINAL.source
                && matches!(ask, yd_core::feed::SourceCmd::Resize { .. })
        });
        assert!(
            sized,
            "the Everything layout's shell is told its size: {asked:?}"
        );
        Ok(())
    }

    #[test]
    fn the_poll_holds_open_every_thread_that_needs_the_user() -> Result<(), Box<dyn Error>> {
        let (win, _) = crate::menubar::tests::window()?;
        let mut runtime = Runtime::new(win, view, (1200.0, 750.0), Env::new(true, 1.0));
        runtime.poll = Some(poll);
        runtime.frame(1.0 / 60.0);
        let hub = runtime.state.hub.try_borrow()?;
        let views = |id: &str| {
            let thread = hub.model.wire.threads.get(&yd_core::ids::ThreadId::new(id));
            thread.map(|thread| thread.views)
        };
        let held = [views("rate-limit"), views("cold-start")];
        assert!(
            held.iter()
                .all(|views| views.is_some_and(|views| views > 0)),
            "the bell keeps both attached, shown or not: {held:?}"
        );
        Ok(())
    }
}
