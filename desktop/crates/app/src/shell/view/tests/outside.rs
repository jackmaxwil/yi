//! Wave 8 joined to the running window: the web page, plugins, containers and services through
//! the app's root. Sources run through injected runners on recorded output; nothing real starts.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use yd_core::command::tools::{container, plugin, remote, service};
use yd_core::feed::{
    CONTAINERS, DAEMONS, FILES, FeedEvent, Pointer, REMOTE, REPORT, ScreenInput, SourceCmd,
    TERMINAL, WEB,
};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{FeedId, PaneId, RowId, SettingKey};
use yd_core::inbox::inbox;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Value};
use yd_core::settings::schema::SettingValue;
use yd_core::settings::store::SettingsMsg;
use yd_panes::assets::desc::{OPEN_SHELL, SHOW_FINDINGS};
use yd_panes::daemons::pane::LABELS;
use yd_panes::remote::pane::ADAPTER;
use yd_panes::vms::desc::OPEN_SCREEN;
use yd_sources::{docker, launchd, machines};
use yd_ui::grid_view::picker;
use yd_ui::menu::command_box;
use yd_ui::win::{dispatch, field};

use super::*;
use crate::start::{self, Files};

/// A frame Chrome wrote: a 40 by 30 page.
const FRAME: &[u8] = include_bytes!("../../../../../ui/src/screen/surface/quad.png");
/// A plugin that counts its runs in its store and shows the count.
const COUNTER: &str = r#"
local pane = require("@yi/pane")
return { view = function(ctx)
    local n = (ctx.store.get("runs") or 0) + 1
    ctx.store.set("runs", n)
    return { pane.stat("Runs", n) }
end }
"#;
/// `launchctl list`, as recorded on a Mac that runs Yi's server.
const SERVICES: &str = "PID\tStatus\tLabel\n4412\t0\tcom.yi.serve\n-\t0\tcom.apple.Finder\n";

/// What a window asked of its sources, in order.
type Asked = Rc<RefCell<Vec<(FeedId, SourceCmd)>>>;

fn asked(runtime: &Runtime<Win>) -> Result<Asked, Box<dyn Error>> {
    let asks = Asked::default();
    let log = Rc::clone(&asks);
    runtime.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let Effect::Source(feed, ask) = effect {
            log.borrow_mut().push((feed, ask));
        }
    });
    Ok(asks)
}

/// The scene `name` at rest, with every ask of a source the window makes from here on.
fn sourced(name: &str) -> Result<(Runtime<Win>, Asked), Box<dyn Error>> {
    let mut runtime = unframed(name)?;
    rest(&mut runtime);
    let asks = asked(&runtime)?;
    Ok((runtime, asks))
}

fn run(runtime: &mut Runtime<Win>, run: Run) {
    runtime.update(move |win, tx| {
        command_box::run(win, tx, run.clone());
    });
    rest(runtime);
}

/// The pane of `kind` the main window draws.
fn pane_of(runtime: &Runtime<Win>, kind: PaneKind) -> Result<PaneId, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let window = grid.window(root::MAIN).ok_or("no window")?;
    let mut docked = window.layout.docked().map(|(pane, _)| pane);
    let found = docked.find(|pane| grid.kind(*pane) == Some(kind));
    found.ok_or_else(|| format!("the window draws no {kind:?} pane").into())
}

fn pane_key(runtime: &Runtime<Win>, kind: PaneKind) -> Result<String, Box<dyn Error>> {
    Ok(format!("{}-{}", kind.slug(), pane_of(runtime, kind)?.0))
}

/// The words drawn inside the element keyed `key`.
fn inside(runtime: &Runtime<Win>, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, key).ok_or_else(|| format!("no {key}"))?.rect;
    let within =
        |r: &[f32; 4]| r[0] >= x - 0.5 && r[1] >= y - 0.5 && r[1] <= y + h && r[0] <= x + w;
    let live = shown.iter().filter(|e| !e.exiting && within(&e.rect));
    Ok(live.filter_map(|e| e.text.clone()).collect())
}

fn thing(kind: Kind, id: &str) -> Result<Target, Box<dyn Error>> {
    Ok(Target::Thing(Address::new(kind, id)?))
}

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/in8")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The fixture rows of the four list panes and a plugin, as their sources would have sent.
fn filled(runtime: &mut Runtime<Win>) -> Check {
    let mut hub = runtime.state.hub.try_borrow_mut()?;
    let scenes = [
        "assets/mockup",
        "containers/mockup",
        "vms/mockup",
        "daemons/mockup",
        "plugins/live",
    ];
    command_box::borrow(&mut hub, &scenes);
    Ok(())
}

#[test]
fn each_of_the_nine_new_panes_is_shown_by_its_show_command_and_draws_a_body() -> Check {
    let panes = [
        (PaneKind::Web, "Storefront"),
        (PaneKind::Report, ""),
        (PaneKind::Script, "Queue depth"),
        (PaneKind::Plugins, "queue-depth"),
        (PaneKind::PluginKit, "Pieces a pane can use"),
        (PaneKind::Assets, "hv-01"),
        (PaneKind::Containers, "atlas-api"),
        (PaneKind::Vms, "ci-ubuntu"),
        (PaneKind::Daemons, "yi serve"),
    ];
    for (kind, part) in panes {
        let mut runtime = unframed("shell/everything")?;
        filled(&mut runtime)?;
        rest(&mut runtime);
        let show = picker::show(kind).ok_or_else(|| format!("no show command for {kind:?}"))?;
        run(&mut runtime, Run::new(show));
        let key = pane_key(&runtime, kind)?;
        let words = inside(&runtime, &key)?;
        assert!(!words.is_empty(), "{kind:?} draws a blank body");
        let said = words.iter().any(|said| said.contains(part));
        assert!(said, "{kind:?} does not say {part:?}: {words:?}");
    }
    Ok(())
}

#[test]
fn every_setting_a_pane_declares_is_listed_from_the_start() -> Check {
    let win = root::init();
    let hub = win.hub.try_borrow()?;
    for key in ["plugin-kit.grants", LABELS] {
        let key = SettingKey::fixed(key);
        let listed = hub.model.settings.rows.iter().any(|row| row.key == key);
        assert!(listed, "{key:?} is not declared at start");
    }
    Ok(())
}

/// Every ask a window made of `feed`, handed to a source's loop as its commands.
fn commands(asks: &Asked, feed: FeedId) -> std::sync::mpsc::Receiver<SourceCmd> {
    let (tx, rx) = channel();
    for (to, ask) in asks.borrow().iter() {
        if *to == feed {
            tx.send(ask.clone()).ok();
        }
    }
    rx
}

/// What the Docker source ran for the window's asks, less its own readings.
fn docker_ran(asks: &Asked) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let calls = RefCell::new(Vec::new());
    let runner = |args: &[String]| {
        if !matches!(args.first().map(String::as_str), Some("ps" | "stats")) {
            calls.borrow_mut().push(args.to_vec());
        }
        Ok(docker::Ran {
            ok: true,
            ..docker::Ran::default()
        })
    };
    let (tx, _inbox) = inbox(|| {});
    let cmds = commands(asks, CONTAINERS);
    docker::serve(
        &tx.feed(CONTAINERS),
        &cmds,
        &runner,
        Duration::from_secs(60),
    )?;
    Ok(calls.take())
}

#[test]
fn stop_on_a_container_asks_first_and_only_yes_sends_docker_stop_with_its_name() -> Check {
    let (mut runtime, asks) = sourced("containers/mockup")?;
    let stop = "containers-0.detail.atlas-api.container.stop";
    tap(&mut runtime, stop)?;
    let question = row_text(&runtime.elements(), "containers.ask")?;
    assert!(question.starts_with("Stop atlas-api?"), "{question}");
    assert_eq!(docker_ran(&asks)?, Vec::<Vec<String>>::new(), "ran unasked");
    tap(&mut runtime, "containers.ask.no")?;
    assert!(!live(&runtime, "containers.ask"), "the question stays");
    assert_eq!(docker_ran(&asks)?, Vec::<Vec<String>>::new(), "a no ran it");
    tap(&mut runtime, stop)?;
    tap(&mut runtime, "containers.ask.yes")?;
    assert_eq!(docker_ran(&asks)?, [["stop", "atlas-api"]]);
    Ok(())
}

#[test]
fn a_containers_log_lines_are_the_open_containers_output_and_never_rows() -> Check {
    let (mut runtime, _) = sourced("containers/mockup")?;
    let before = row_text(&runtime.elements(), "containers-0.pane-tab-bar")?;
    let line = |n: u32, words: &str| {
        let mut row = Row::new(format!("log:atlas-api:{n}")).text("last_line", words);
        row.parent = Some(RowId::new("atlas-api"));
        row.dim = true;
        row
    };
    let lines = vec![line(1, "listening on :8080"), line(2, "GET /health 200")];
    runtime.update(move |win, _| {
        for row in lines.clone() {
            dispatch(win, Msg::Feed(CONTAINERS, FeedEvent::Upsert(row)));
        }
    });
    rest(&mut runtime);
    assert!(
        !live(&runtime, "containers-0.row.log:atlas-api:1"),
        "a log line is listed as a container"
    );
    let output = inside(&runtime, "containers-0.detail.atlas-api")?.join("\n");
    assert!(
        output.contains("listening on :8080\nGET /health 200"),
        "{output}"
    );
    let after = row_text(&runtime.elements(), "containers-0.pane-tab-bar")?;
    assert_eq!(after, before, "the tab counts log lines as containers");
    Ok(())
}

#[test]
fn the_host_picker_and_the_other_views_say_they_are_not_available_when_pressed() -> Check {
    let (mut runtime, asks) = sourced("containers/mockup")?;
    for (chip, said) in [
        ("images", "Images is not available yet"),
        ("volumes", "Volumes is not available yet"),
        ("host", "Choosing a Docker host is not available yet"),
    ] {
        tap(&mut runtime, &format!("containers-0.{chip}"))?;
        assert_eq!(row_text(&runtime.elements(), "containers-0.said")?, said);
    }
    assert!(asks.borrow().is_empty(), "{:?}", asks.borrow());
    Ok(())
}

/// Answers `launchctl` and `id` with recorded output and keeps what it was asked.
fn launchctl(
    calls: &RefCell<Vec<String>>,
) -> impl FnMut(&str, &[&str]) -> Result<Vec<u8>, launchd::RunError> + '_ {
    move |program, args| {
        let said = format!("{program} {}", args.join(" "));
        calls.borrow_mut().push(said);
        Ok(match (program, args) {
            ("launchctl", ["list"]) => SERVICES.as_bytes().to_vec(),
            ("id", ["-u"]) => b"501\n".to_vec(),
            _ => Vec::new(),
        })
    }
}

/// The services as the launchd source reads them, in the pane's table.
fn services(runtime: &mut Runtime<Win>) -> Check {
    let calls = RefCell::new(Vec::new());
    let (scope, mut seen) = (launchd::Scope::default(), launchd::Plists::default());
    let reading = launchd::read(&mut launchctl(&calls), &scope, launchd::Uid(501), &mut seen)?;
    runtime.update(move |win, _| {
        let rows = FeedEvent::Reset(reading.rows.clone());
        dispatch(win, Msg::Feed(DAEMONS, rows));
    });
    rest(runtime);
    Ok(())
}

#[test]
fn restart_on_a_service_sends_the_launchctl_kickstart_line_for_that_service() -> Check {
    let (mut runtime, asks) = sourced("daemons/stub")?;
    services(&mut runtime)?;
    tap(&mut runtime, "daemons-0.row.com.yi.serve")?;
    tap(
        &mut runtime,
        "daemons-0.detail.com.yi.serve.service.restart",
    )?;
    let calls = RefCell::new(Vec::new());
    let (tx, _inbox) = inbox(|| {});
    let cmds = commands(&asks, DAEMONS);
    launchd::serve(
        tx.feed(DAEMONS),
        cmds,
        &mut launchctl(&calls),
        launchd::Scope::default(),
        Duration::from_secs(60),
    )?;
    let calls = calls.take();
    let acted: Vec<&String> = calls.iter().filter(|c| c.contains("kickstart")).collect();
    assert_eq!(acted, ["launchctl kickstart -k gui/501/com.yi.serve"]);
    Ok(())
}

#[test]
fn stopping_a_service_that_runs_at_login_says_it_starts_again_at_the_next_login() -> Check {
    let (mut runtime, asks) = sourced("daemons/stub")?;
    let serve = Row::new("com.yi.serve")
        .text("name", "com.yi.serve")
        .cell("login", Value::Flag(true))
        .cell("actions", Value::Tags(vec!["Stop".to_owned()]));
    let mut line = Row::new("log:com.yi.serve:1").text("name", "listening on the socket");
    (line.parent, line.dim) = (Some(RowId::new("com.yi.serve")), true);
    runtime.update(move |win, _| {
        let rows = FeedEvent::Reset(vec![serve.clone(), line.clone()]);
        dispatch(win, Msg::Feed(DAEMONS, rows));
    });
    rest(&mut runtime);
    // A log line is drawn under its service, indented and dimmed, and is not counted as one.
    let shown = runtime.elements();
    let said = row_text(&shown, "daemons-0.row.log:com.yi.serve:1")?;
    assert_eq!(said, "listening on the socket");
    let parent = by_text(&shown, "com.yi.serve").ok_or("no service row")?;
    let line = by_text(&shown, "listening on the socket").ok_or("no line")?;
    assert_eq!(
        (parent.opacity, line.opacity),
        (1.0, 0.5),
        "a log line is dimmed"
    );
    assert!(
        line.rect[0] > parent.rect[0],
        "the line hangs under its service"
    );
    let head = row_text(&shown, "daemons-0.pane-header")?;
    assert!(head.contains("0 running"), "{head}");

    tap(&mut runtime, "daemons-0.row.com.yi.serve")?;
    tap(&mut runtime, "daemons-0.detail.com.yi.serve.service.stop")?;
    let question = row_text(&runtime.elements(), "daemons-0.ask")?;
    assert!(
        question.contains("starts again the next time you log in"),
        "{question}"
    );
    assert!(asks.borrow().is_empty(), "sent before the answer");
    tap(&mut runtime, "daemons-0.ask.yes")?;
    let sent = asks.borrow();
    let stopped = matches!(
        sent.as_slice(),
        [(DAEMONS, SourceCmd::Run(run))] if run.command == service::STOP
    );
    assert!(stopped, "{sent:?}");
    Ok(())
}

fn by_text<'a>(elements: &'a [ElementInfo], words: &str) -> Option<&'a ElementInfo> {
    let live = elements.iter().filter(|e| !e.exiting);
    live.into_iter().find(|e| e.text.as_deref() == Some(words))
}

#[test]
fn a_pane_coming_and_going_tells_its_source_whether_it_is_seen() -> Check {
    let (mut runtime, asks) = sourced("shell/everything")?;
    let labels = SettingValue::Text(Cow::Borrowed("com.example.other\n\n com.example.more "));
    runtime.update(move |win, _| {
        let set = SettingsMsg::Set(SettingKey::fixed(LABELS), Some(labels.clone()));
        dispatch(win, Msg::Settings(set));
    });
    rest(&mut runtime);
    let polled = |asks: &Asked| -> Vec<(FeedId, SourceCmd)> {
        let all = asks.borrow();
        let ours = all
            .iter()
            .filter(|(feed, _)| [CONTAINERS, DAEMONS].contains(feed));
        ours.cloned().collect()
    };
    assert_eq!(polled(&asks), [], "a source was told with no pane open");
    for kind in [PaneKind::Containers, PaneKind::Daemons] {
        let show = picker::show(kind).ok_or("no show command")?;
        run(&mut runtime, Run::new(show));
    }
    let named = ["com.example.other", "com.example.more"].map(str::to_owned);
    // Daemons opens as a tab in front of Containers, in the same place: Containers is
    // behind it then, and its source is told it is not seen.
    let opened = [
        (CONTAINERS, SourceCmd::Seen(true)),
        (DAEMONS, SourceCmd::Labels(named.to_vec())),
        (CONTAINERS, SourceCmd::Seen(false)),
        (DAEMONS, SourceCmd::Seen(true)),
    ];
    assert_eq!(polled(&asks), opened);
    let containers = {
        let hub = runtime.state.hub.try_borrow()?;
        let mut panes = hub.model.grid.panes.iter();
        let found = panes.find(|pane| pane.kind == PaneKind::Containers);
        found.map(|pane| pane.id).ok_or("no containers pane")?
    };
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Front(containers))));
    rest(&mut runtime);
    let mut turned = polled(&asks).split_off(opened.len());
    turned.sort_by_key(|(feed, _)| feed.source);
    let front = [
        (CONTAINERS, SourceCmd::Seen(true)),
        (DAEMONS, SourceCmd::Seen(false)),
    ];
    assert_eq!(turned, front, "its tab in front, each source is told once");
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Close(containers))));
    rest(&mut runtime);
    let mut closed = polled(&asks).split_off(opened.len() + front.len());
    closed.sort_by_key(|(feed, _)| feed.source);
    let after = [
        (CONTAINERS, SourceCmd::Seen(false)),
        (DAEMONS, SourceCmd::Seen(true)),
    ];
    assert_eq!(
        closed, after,
        "closed, Daemons has the slot and is seen again"
    );
    Ok(())
}

#[test]
fn closing_the_last_web_or_report_pane_stops_its_browser() -> Check {
    let (mut runtime, asks) = sourced("shell/everything")?;
    for kind in [PaneKind::Web, PaneKind::Report] {
        let show = picker::show(kind).ok_or("no show command")?;
        run(&mut runtime, Run::new(show));
    }
    let closes = |asks: &Asked| -> Vec<FeedId> {
        let all = asks.borrow();
        let closed = all.iter().filter(|(_, ask)| *ask == SourceCmd::Close);
        closed.map(|(feed, _)| *feed).collect()
    };
    assert_eq!(closes(&asks), [], "a page was closed while shown");
    for kind in [PaneKind::Web, PaneKind::Report] {
        while let Ok(pane) = pane_of(&runtime, kind) {
            runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Close(pane))));
            rest(&mut runtime);
        }
    }
    assert_eq!(closes(&asks), [WEB, REPORT]);
    Ok(())
}

/// A window whose plugin folder is the test's own, holding one plugin, with its Script pane
/// shown.
fn plugged(name: &str) -> Result<(Runtime<Win>, Asked, PathBuf), Box<dyn Error>> {
    let dir = scratch(name)?;
    let plugin = dir.join("panes/counter");
    std::fs::create_dir_all(&plugin)?;
    std::fs::write(plugin.join("plugin.json"), r#"{"name": "counter"}"#)?;
    std::fs::write(plugin.join("pane.luau"), COUNTER)?;
    let mut runtime = unframed("shell/everything")?;
    start::files(
        &mut *runtime.state.hub.try_borrow_mut()?,
        Files::under(&dir),
    );
    let asks = asked(&runtime)?;
    let show = picker::show(PaneKind::Script).ok_or("no show command")?;
    run(&mut runtime, Run::new(show));
    Ok((runtime, asks, dir))
}

#[test]
fn a_plugin_folder_loads_draws_its_stat_and_a_changed_file_redraws_with_its_state_kept() -> Check {
    let (mut runtime, _, dir) = plugged("plugin")?;
    let key = pane_key(&runtime, PaneKind::Script)?;
    let words = inside(&runtime, &key)?;
    assert!(words.iter().any(|said| said == "Runs"), "{words:?}");
    assert!(words.contains(&"live · version 1".to_owned()), "{words:?}");
    let source = dir.join("panes/counter/pane.luau");
    let before = std::fs::metadata(&source)?.modified()?;
    std::fs::write(&source, COUNTER.replace("\"Runs\"", "\"Runs so far\""))?;
    let file = std::fs::File::options().write(true).open(&source)?;
    file.set_modified(before + Duration::from_secs(2))?;
    // The folder is looked at once a second of the window's own clock.
    runtime.frame(1.2);
    rest(&mut runtime);
    let words = inside(&runtime, &key)?;
    assert!(words.iter().any(|said| said == "Runs so far"), "{words:?}");
    assert!(words.contains(&"live · version 2".to_owned()), "{words:?}");
    let count = words
        .iter()
        .filter_map(|said| said.parse::<u32>().ok())
        .max();
    assert!(count >= Some(2), "the count started again: {words:?}");
    start::quit(&mut runtime.state);
    Ok(())
}

#[test]
fn open_source_on_a_plugins_number_reads_its_file_from_the_panes_folder() -> Check {
    let (mut runtime, asks, dir) = plugged("source")?;
    let number = thing(Kind::Number, "counter/runs")?;
    run(&mut runtime, Run::new(plugin::OPEN_SOURCE).on(number));
    let canon = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let files: Vec<SourceCmd> = {
        let all = asks.borrow();
        let ours = all.iter().filter(|(feed, _)| *feed == FILES);
        ours.map(|(_, ask)| ask.clone()).collect()
    };
    let panes = canon(&dir.join("panes"));
    let source = canon(&dir.join("panes/counter/pane.luau"));
    let named = files.iter().any(|ask| match ask {
        SourceCmd::Readable(folder) => canon(folder) == panes,
        _ => false,
    });
    let read = files.iter().any(|ask| match ask {
        SourceCmd::Read(path) => canon(path) == source,
        _ => false,
    });
    assert!(named && read, "{files:?}");
    start::quit(&mut runtime.state);
    Ok(())
}

/// Through the box itself: opened with its key, the command found by its name and run with
/// Return, and the words typed next land in the kit's field.
#[test]
fn build_a_pane_in_the_command_box_shows_the_kit_with_its_field_holding_the_keyboard() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let key = |words: &str| Key::Text(words.to_owned());
    strike(&mut runtime, key("k"), cmd());
    strike(&mut runtime, key("Build a pane"), Mods::default());
    strike(&mut runtime, Key::Enter, Mods::default());
    assert!(!live(&runtime, "window.command-box"), "the box stays open");
    let kit = pane_of(&runtime, PaneKind::PluginKit)?;
    strike(&mut runtime, key("a pane of queue depth"), Mods::default());
    let typed = runtime.state.edits.get(&field(kit, "want"));
    let typed = typed.map(|field| field.text.clone());
    assert_eq!(typed.as_deref(), Some("a pane of queue depth"));
    Ok(())
}

#[test]
fn a_typed_address_opens_a_pushed_frame_is_drawn_and_a_press_reaches_the_page() -> Check {
    let (mut runtime, asks) = sourced("shell/everything")?;
    let web = pane_of(&runtime, PaneKind::Web)?;
    let key = format!("web-{}", web.0);
    tap(&mut runtime, &format!("{key}.address.input"))?;
    let all = Mods {
        cmd: true,
        logo: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Mods::default()
    };
    strike(&mut runtime, Key::Command('a'), all);
    strike(
        &mut runtime,
        Key::Text("localhost:5173/upload".to_owned()),
        Mods::default(),
    );
    strike(&mut runtime, Key::Enter, Mods::default());
    let opened: Vec<String> = {
        let all = asks.borrow();
        let runs = all.iter().filter_map(|(feed, ask)| match ask {
            SourceCmd::Run(run) if *feed == WEB => Some(run.clone()),
            _ => None,
        });
        runs.filter_map(|run| run.text).collect()
    };
    assert_eq!(opened, ["http://localhost:5173/upload"]);

    // The source answers with the page's row and a frame, pushed from its own thread.
    let page = Row::new("page:0")
        .text("url", "http://localhost:5173/upload")
        .text("title", "localhost:5173");
    runtime.update(move |win, _| dispatch(win, Msg::Feed(WEB, FeedEvent::Upsert(page.clone()))));
    rest(&mut runtime);
    let screen = {
        let hub = runtime.state.hub.try_borrow()?;
        let state = hub.panes.get::<yd_panes::web::pane::State>(Some(web));
        state.ok_or("the pane keeps no screen")?.screen.clone()
    };
    let pushed = std::thread::spawn(move || screen.push_png(FRAME));
    pushed.join().map_err(|_| "the pushing thread died")??;
    rest(&mut runtime);
    let surface = format!("{key}.page");
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, &surface).ok_or("no page surface")?.rect;
    let drawn = runtime.instances.iter().any(|drawn| {
        let r = drawn.rect;
        r[2] > 1.0
            && (r[0] - x).abs() < 0.5
            && (r[1] - y).abs() < 0.5
            && r[2] <= w + 0.5
            && (r[2] / r[3] - 40.0 / 30.0).abs() < 0.01
    });
    assert!(
        drawn,
        "no picture of the frame's shape is drawn in the page's box"
    );

    asks.borrow_mut().clear();
    let scale = (w / 40.0).min(h / 30.0);
    let at = (x + 20.5 * scale, y + 15.5 * scale);
    runtime.pointer_move(at.0, at.1);
    runtime.pointer_down(at.0, at.1);
    runtime.pointer_up(at.0, at.1);
    rest(&mut runtime);
    let press = ScreenInput::Pointer {
        phase: Pointer::Press,
        x: 20,
        y: 15,
    };
    let sent = asks.borrow();
    assert!(sent.contains(&(WEB, SourceCmd::Screen(press))), "{sent:?}");
    Ok(())
}

/// The window in the saved layout `saved`, with the four list panes' rows in their tables.
fn laid_out(saved: &'static str) -> Result<(Runtime<Win>, Asked), Box<dyn Error>> {
    let mut runtime = unframed("shell/everything")?;
    filled(&mut runtime)?;
    runtime.update(move |win, _| {
        let window = root::MAIN;
        dispatch(win, Msg::Grid(GridMsg::Show { window, saved }));
    });
    rest(&mut runtime);
    let asks = asked(&runtime)?;
    Ok((runtime, asks))
}

/// Each button shows the pane it names in a layout that draws none; the container's two also
/// reach what is behind them, and nothing is typed for an asset.
#[test]
fn the_buttons_that_name_a_pane_show_it_aimed_at_the_row() -> Check {
    let build = thing(Kind::Asset, "build-01")?;
    let api = thing(Kind::Container, "atlas-api")?;
    let cases = [
        ("screens", OPEN_SHELL, PaneKind::Terminal, build.clone()),
        ("screens", container::SHELL, PaneKind::Terminal, api),
        (
            "code",
            OPEN_SCREEN,
            PaneKind::Remote,
            thing(Kind::Vm, "ci-ubuntu")?,
        ),
        ("code", SHOW_FINDINGS, PaneKind::Findings, build),
    ];
    for (saved, button, kind, row) in cases {
        let (mut runtime, asks) = laid_out(saved)?;
        assert!(
            pane_of(&runtime, kind).is_err(),
            "{kind:?} was there already"
        );
        run(&mut runtime, Run::new(button).on(row));
        let key = pane_key(&runtime, kind)?;
        assert!(live(&runtime, &key), "{button:?} does not draw {kind:?}");
        let asks = asks.borrow();
        let typed = asks.iter().find_map(|(feed, ask)| match ask {
            SourceCmd::Input(line) if feed.source == TERMINAL.source => Some(line.as_slice()),
            _ => None,
        });
        let exec = &b"docker exec -it atlas-api sh\r"[..];
        assert_eq!(typed, (button == container::SHELL).then_some(exec));
    }
    Ok(())
}

/// The Remote pane in the real window on recorded data: its asks reach the source, what the
/// source sends is drawn, and a press in Control goes out as a point of the screen.
#[test]
fn the_remote_pane_asks_its_source_draws_what_it_sends_and_control_reaches_it() -> Check {
    let mut runtime = unframed("shell/everything")?;
    let asks = asked(&runtime)?;
    runtime.update(|win, _| {
        let recorded = SettingValue::Text(Cow::Borrowed("recorded"));
        let set = SettingsMsg::Set(SettingKey::fixed(ADAPTER), Some(recorded));
        dispatch(win, Msg::Settings(set));
    });
    rest(&mut runtime);
    let ran = |asks: &Asked| -> Vec<(CommandId, Option<String>)> {
        let all = asks.borrow();
        let runs = all.iter().filter_map(|(feed, ask)| match ask {
            SourceCmd::Run(run) if *feed == REMOTE => Some((run.command, run.text.clone())),
            _ => None,
        });
        runs.collect()
    };
    let told = ran(&asks);
    assert!(
        told.contains(&(remote::ADAPTER, Some("recorded".to_owned()))),
        "{told:?}"
    );
    assert!(told.contains(&(remote::OPEN, None)), "{told:?}");

    let pane = pane_of(&runtime, PaneKind::Remote)?;
    let screen = {
        let hub = runtime.state.hub.try_borrow()?;
        let state = hub.panes.get::<yd_panes::remote::pane::State>(Some(pane));
        state.ok_or("the pane keeps no screen")?.screen.clone()
    };
    let (tx, inbox) = inbox(|| {});
    let show = |w, h, rgba| screen.push(w, h, rgba).map_err(|why| why.to_string());
    machines::remote_screen(tx.feed(REMOTE), commands(&asks, REMOTE), &show)?;
    inbox.drain(&mut runtime.state.hub.try_borrow_mut()?.model);
    runtime.update(|_, _| {});
    rest(&mut runtime);
    let key = pane_key(&runtime, PaneKind::Remote)?;
    assert!(
        live(&runtime, &format!("{key}.screen.window:terminal")),
        "no windows drawn"
    );

    tap(&mut runtime, &format!("{key}.mode.1"))?;
    asks.borrow_mut().clear();
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, &format!("{key}.surface"))
        .ok_or("no surface")?
        .rect;
    let at = (x + w * 0.25, y + h * 0.75);
    runtime.pointer_move(at.0, at.1);
    runtime.pointer_down(at.0, at.1);
    runtime.pointer_up(at.0, at.1);
    rest(&mut runtime);
    let pressed = asks.borrow().iter().find_map(|(feed, ask)| match ask {
        SourceCmd::Screen(ScreenInput::Pointer {
            phase: Pointer::Press,
            x,
            y,
        }) if *feed == REMOTE => Some((*x, *y)),
        _ => None,
    });
    let (px, py) = pressed.ok_or("a press in Control did not reach the source")?;
    assert!(
        px.abs_diff(215) <= 1 && py.abs_diff(363) <= 1,
        "({px}, {py})"
    );

    tap(&mut runtime, &format!("{key}.screenshot"))?;
    let window = {
        let hub = runtime.state.hub.try_borrow()?;
        let taken = hub.model.attached.things.iter();
        let taken: Vec<String> = taken.map(|t| t.address.to_string()).collect();
        assert_eq!(taken, ["yi://screen/ci-ubuntu#t=17:58"]);
        let found = yd_ui::overlay::pick::thing_at(&hub, &format!("{key}.screen.window:terminal"));
        found.ok_or("the window is no thing")?.address
    };
    // The right-click row "Mark it up" on that window puts the pane's bar in Mark up.
    let mark = yd_core::command::by_kind::SCREEN_MARK;
    runtime.update(move |win, tx| {
        view::run(win, tx, mark, Target::Thing(window.clone()));
    });
    rest(&mut runtime);
    let hub = runtime.state.hub.try_borrow()?;
    let state = hub.panes.get::<yd_panes::remote::pane::State>(Some(pane));
    let mode = state.map(|state| state.mode);
    assert_eq!(mode, Some(yd_panes::remote::pane::Mode::MarkUp));
    Ok(())
}

#[test]
fn open_screen_on_a_vm_asks_the_source_for_that_machine() -> Check {
    let (mut runtime, asks) = laid_out("code")?;
    let machine = thing(Kind::Vm, "db-01")?;
    run(&mut runtime, Run::new(OPEN_SCREEN).on(machine.clone()));
    let all = asks.borrow();
    let opened = all.iter().any(|(feed, ask)| {
        matches!(ask, SourceCmd::Run(run) if *feed == REMOTE && run.command == remote::OPEN && run.target == machine)
    });
    assert!(opened, "{all:?}");
    Ok(())
}
