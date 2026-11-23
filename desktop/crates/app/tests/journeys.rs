//! Journeys the real binary cannot run without touching this Mac: recorded outside systems,
//! a scripted debugger, a pane in a second window, and the user's files kept across a restart.

mod support;

use std::cell::RefCell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use rune::{ElementInfo, Env, Runtime};
use yd_core::address::uri::Address;
use yd_core::command::by_kind::FINDING;
use yd_core::command::table::{Run, Target};
use yd_core::feed::SourceCmd;
use yd_core::grid::step::GridMsg;
use yd_core::ids::ThemeId;
use yd_core::ids::WindowId;
use yd_core::inbox::inbox;
use yd_core::kind::Kind;
use yd_core::msg::{Effect, Msg};
use yd_core::pane::PaneKind;
use yd_sources::dap::client::{self as dap, Adapter, Ask, Locate, Origin};
use yd_sources::docker::{self, Ran};
use yd_sources::launchd::{self, Scope};
use yd_ui::grid_view::view::show_pane;
use yd_ui::menu::command_box;
use yd_ui::theme::files::{self, Library};
use yd_ui::win::{Win, dispatch, drain};
use yi_desktop::{root, start};

type Check = Result<(), Box<dyn Error>>;

const PS: &str = concat!(
    r#"{"ID":"a1b2c3d4e5f6","Names":"atlas-api","Image":"atlas/api:rate-limit","State":"running","Status":"Up 14 minutes (healthy)","Ports":"0.0.0.0:8080->8080/tcp","Labels":"com.docker.compose.project=atlas"}"#,
    "\n",
    r#"{"ID":"c3d4e5f6a1b2","Names":"registry","Image":"registry:2","State":"exited","Status":"Exited (137) 2 hours ago","Ports":"","Labels":""}"#,
    "\n",
);
const STATS: &str = r#"{"ID":"a1b2c3d4e5f6","Name":"atlas-api","CPUPerc":"33.52%","MemUsage":"212MiB / 7.67GiB"}
"#;
const LAUNCHCTL: &str = "PID\tStatus\tLabel\n412\t0\tcom.yi.daemon\n-\t0\tcom.yi.figma-mcp\n";

fn window() -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut runtime = Runtime::new(
        root::init(),
        root::view,
        support::scene::WINDOW,
        Env::new(true, 1.0),
    );
    runtime.poll = Some(root::poll);
    runtime.frame(1.0 / 60.0);
    Ok(runtime)
}

fn settle(runtime: &mut Runtime<Win>) {
    for _ in 0..40 {
        runtime.frame(1.0 / 60.0);
    }
}

fn words(runtime: &Runtime<Win>) -> Vec<String> {
    let all: Vec<ElementInfo> = runtime.elements();
    all.into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.text)
        .collect()
}

fn has(runtime: &Runtime<Win>, text: &str) -> bool {
    words(runtime).iter().any(|word| word.contains(text))
}

fn shown(runtime: &mut Runtime<Win>, kind: PaneKind) {
    runtime.update(move |win, tx| {
        show_pane(win, tx, kind);
    });
    settle(runtime);
}

#[test]
fn containers_and_daemons_fill_from_recorded_output_and_only_read() -> Check {
    let mut runtime = window()?;
    let (sender, inbox) = inbox(|| {});
    runtime.state.hub.try_borrow_mut()?.set_inbox(inbox);
    let called: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let docker_says = |args: &[String]| {
        called
            .borrow_mut()
            .push(format!("docker {}", args.join(" ")));
        let out = match args.first().map(String::as_str) {
            Some("ps") => PS,
            Some("stats") => STATS,
            _ => "",
        };
        Ok(Ran {
            ok: true,
            stdout: out.into(),
            stderr: Vec::new(),
        })
    };
    let (tell, cmds) = channel();
    tell.send(SourceCmd::Refresh)?;
    drop(tell);
    docker::serve(
        &sender.feed(docker::FEED),
        &cmds,
        &docker_says,
        Duration::from_secs(60),
    )?;
    let mut launchd_says = |program: &str, args: &[&str]| {
        called
            .borrow_mut()
            .push(format!("{program} {}", args.join(" ")));
        Ok(match (program, args) {
            ("launchctl", ["list"]) => LAUNCHCTL.as_bytes().to_vec(),
            ("id", ["-u"]) => b"501\n".to_vec(),
            _ => Vec::new(),
        })
    };
    let (tell, cmds) = channel();
    tell.send(SourceCmd::Refresh)?;
    drop(tell);
    let scope = Scope::default();
    launchd::serve(
        sender.feed(launchd::FEED),
        cmds,
        &mut launchd_says,
        scope,
        Duration::from_secs(60),
    )?;
    runtime.update(|win, _| {
        drain(win);
    });
    // The two share the side column as tabs: each is read while its tab is in front.
    let drawn = [
        (PaneKind::Containers, ["atlas-api", "registry"]),
        (PaneKind::Daemons, ["com.yi.daemon", "com.yi.figma-mcp"]),
    ];
    for (kind, wanted) in drawn {
        shown(&mut runtime, kind);
        for want in wanted {
            assert!(
                has(&runtime, want),
                "{want} is not drawn: {:?}",
                words(&runtime)
            );
        }
    }
    let moved = [
        "start",
        "stop",
        "rm",
        "restart",
        "bootout",
        "kickstart",
        "disable",
    ];
    let calls = called.borrow();
    assert!(!calls.is_empty(), "the sources ran nothing at all");
    assert!(
        calls
            .iter()
            .all(|call| !call.split_whitespace().any(|word| moved.contains(&word))),
        "a source changed something on this Mac: {calls:?}"
    );
    Ok(())
}

fn here() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../sources/src/dap/client")
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[test]
fn a_breakpoint_stops_the_scripted_adapter_and_the_debugger_shows_it() -> Check {
    let script = leak(here().join("adapter.py").display().to_string());
    let tape = leak(here().join("fixtures/probe.jsonl").display().to_string());
    let args: &'static [&'static str] = Box::leak(vec![script, "replay", tape].into_boxed_slice());
    let adapters: &'static [Adapter<'static>] = Box::leak(Box::new([Adapter {
        name: "LLDB",
        id: "lldb-dap",
        locate: Locate::At("/usr/bin/python3"),
        args,
    }]));
    let mut runtime = window()?;
    let (sender, inbox) = inbox(|| {});
    runtime.state.hub.try_borrow_mut()?.set_inbox(inbox);
    let (asks, cmds) = channel();
    let out = sender.feed(dap::FEED);
    let source = thread::spawn(move || dap::serve(out, cmds, adapters));
    let probe = here().join("probe.c").display().to_string();
    let line = Target::Thing(Address::new(Kind::Lines, probe)?.part("L10"));
    let user = |run: Run| {
        asks.send(Ask {
            cmd: SourceCmd::Run(run),
            from: Origin::User,
        })
    };
    user(Run::new(dap::BREAK).on(line))?;
    user(Run {
        text: Some("probe".to_owned()),
        ..Run::new(dap::LAUNCH).on(Target::App)
    })?;
    shown(&mut runtime, PaneKind::Debugger);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !has(&runtime, "take") {
        assert!(
            Instant::now() < deadline,
            "no stop drawn: {:?}",
            words(&runtime)
        );
        runtime.update(|win, _| {
            drain(win);
        });
        settle(&mut runtime);
        thread::sleep(Duration::from_millis(20));
    }
    for want in ["paused", "left", "self"] {
        assert!(
            has(&runtime, want),
            "{want} is not drawn: {:?}",
            words(&runtime)
        );
    }
    drop(asks);
    source.join().map_err(|_| "the source panicked")??;
    Ok(())
}

type Asked = Rc<RefCell<Vec<Effect>>>;

#[test]
fn a_pane_pulled_out_is_a_second_window_over_one_model_and_docks_back() -> Check {
    let mut runtime = window()?;
    let log = Asked::default();
    let kept = Rc::clone(&log);
    runtime
        .state
        .hub
        .try_borrow_mut()?
        .set_sink(move |effect| kept.borrow_mut().push(effect));
    let pane = {
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let main = grid.window(root::MAIN).ok_or("no main window")?;
        let found = main
            .layout
            .docked()
            .find(|(pane, _)| grid.kind(*pane) == Some(PaneKind::Files));
        found.ok_or("no Files pane")?.0
    };
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Leave(pane))));
    settle(&mut runtime);
    let apart: Option<WindowId> = {
        let hub = runtime.state.hub.try_borrow()?;
        hub.model.grid.window_of(pane)
    };
    let apart = apart
        .filter(|id| *id != root::MAIN)
        .ok_or("the pane stayed in the main window")?;
    let opened = log
        .borrow()
        .iter()
        .any(|effect| matches!(effect, Effect::Window(_)));
    assert!(
        opened,
        "pulling a pane out asked the window layer for a window"
    );
    let mut second = Runtime::new(
        Win::new(runtime.state.hub.clone(), apart),
        root::view,
        (720.0, 480.0),
        Env::new(true, 1.0),
    );
    second.poll = Some(root::poll);
    settle(&mut second);
    assert!(
        has(&second, "Files") || !words(&second).is_empty(),
        "the second window draws"
    );
    runtime.update(|win, _| {
        dispatch(
            win,
            Msg::Command(
                yd_core::command::table::Route::Model,
                Run::new(yd_core::command::table::FLIP_CHAT),
            ),
        )
    });
    let side = |rt: &Runtime<Win>| rt.state.hub.try_borrow().map(|hub| hub.model.grid.side);
    assert_eq!(
        side(&runtime)?,
        side(&second)?,
        "both windows hold one model"
    );
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Rejoin(pane))));
    settle(&mut runtime);
    let hub = runtime.state.hub.try_borrow()?;
    assert_eq!(
        hub.model.grid.window_of(pane),
        Some(root::MAIN),
        "the pane docked back"
    );
    assert_eq!(hub.model.grid.origin_of(pane), None);
    Ok(())
}

#[test]
fn theme_and_triage_made_in_the_window_are_as_left_after_a_restart() -> Check {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/journey-restart")
        .join(std::process::id().to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("themes"))?;
    let started = |folder: &Path| -> Result<Runtime<Win>, Box<dyn Error>> {
        let win = root::init();
        start::files(&mut *win.hub.try_borrow_mut()?, start::Files::under(folder));
        let mut runtime =
            Runtime::new(win, root::view, support::scene::WINDOW, Env::new(true, 1.0));
        runtime.poll = Some(root::poll);
        settle(&mut runtime);
        Ok(runtime)
    };
    let mut first = started(&dir)?;
    let file = Target::Thing(Address::new(Kind::File, "src/lib.rs")?);
    first.update(move |win, tx| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(library) = files::library(&mut hub)
        {
            library.pick(&ThemeId::new("ink"));
        }
        files::apply(win, Some(tx));
        command_box::run(win, tx, Run::new(FINDING).on(file));
    });
    settle(&mut first);
    start::quit(&mut first.state);
    let again = started(&dir)?;
    let hub = again.state.hub.try_borrow()?;
    let active = hub.panes.get::<Library>(None).map(|l| l.active.to_string());
    assert_eq!(
        active.as_deref(),
        Some("ink"),
        "the theme in use is the theme after a restart"
    );
    assert_eq!(
        hub.model.findings.triage.made.len(),
        1,
        "the finding made is still there"
    );
    Ok(())
}
