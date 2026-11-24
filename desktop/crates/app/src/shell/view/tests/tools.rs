//! Wave 7 joined to the running window: the findings hub, the language server's marks and
//! answers in the code viewer, the references peek and the debugger, through the app's root.

use std::path::{Path, PathBuf};

use rune::PhysicalInput;
use rune::render::{KIND_GLYPH, KIND_SEGMENT};
use yd_core::command::by_kind::{
    BREAK_HERE, PROBLEM_FIX, PROBLEM_OPEN, REFERENCES, SERVER_LOGS, VARIABLE_WATCH,
};
use yd_core::feed::{DEBUGGER, FeedEvent, LSP, OPEN_FILES, SourceCmd};
use yd_core::findings::state::Stage;
use yd_core::findings::triage::{self, Decision, FindingId};
use yd_core::findings::view::{Scope, findings};
use yd_core::ids::ThreadId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Rows, Value};
use yd_panes::code_view::refs;
use yd_panes::debugger::desc::WATCH_VARIABLE;
use yd_panes::lsp::desc::{OPEN_FINDING, SETTINGS};
use yd_ui::code::doc::LineNo;
use yd_ui::code::view::reveal;
use yd_ui::menu::command_box;
use yd_ui::win::dispatch;

use super::*;
use crate::start::{self, Files};

const FILE: &str = "src/middleware/rate_limit.rs";
const UNLIMITED: &str = "Limit is keyed by user id only, anonymous uploads are unlimited";

/// What a window asked of its sources, as `<feed> <command> <target> <text>`.
type Asked = Rc<RefCell<Vec<String>>>;

fn asked(runtime: &Runtime<Win>) -> Result<Asked, Box<dyn Error>> {
    let asks = Asked::default();
    let log = Rc::clone(&asks);
    runtime.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let Effect::Source(feed, SourceCmd::Run(run)) = effect {
            let at = match &run.target {
                Target::Thing(at) => at.to_string(),
                Target::App | Target::Pane(_) => String::new(),
            };
            let text = run.text.as_deref().unwrap_or_default();
            let said = format!("{} {} {at} {text}", feed.source, run.command.0);
            log.borrow_mut().push(said.trim_end().to_owned());
        }
    });
    Ok(asks)
}

/// The scene `name` at rest, with every ask of a source the window makes from here on.
fn tooled(name: &str) -> Result<(Runtime<Win>, Asked), Box<dyn Error>> {
    let mut runtime = unframed(name)?;
    rest(&mut runtime);
    let asks = asked(&runtime)?;
    Ok((runtime, asks))
}

/// Rows a source sends, folded in as its feed's events are.
fn fed(runtime: &mut Runtime<Win>, feed: yd_core::ids::FeedId, rows: Vec<Row>) {
    runtime.update(move |win, _| {
        for row in rows.clone() {
            dispatch(win, Msg::Feed(feed, FeedEvent::Upsert(row)));
        }
    });
    rest(runtime);
}

fn painted(runtime: &Runtime<Win>, kind: f32, row: &str) -> Result<Vec<[f32; 4]>, Box<dyn Error>> {
    let shown = runtime.elements();
    let [_, y, _, h] = by_key(&shown, row).ok_or_else(|| format!("no {row}"))?.rect;
    let inside = |r: &[f32; 4]| r[1] + r[3] / 2.0 >= y && r[1] + r[3] / 2.0 <= y + h;
    let kinds = runtime.instances.iter().filter(|i| i.params[1] == kind);
    let mut found: Vec<[f32; 4]> = kinds.map(|i| i.rect).filter(inside).collect();
    found.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(found)
}

/// The key of the code viewer the window draws: the scene's pane, or the grid's.
fn code(runtime: &Runtime<Win>) -> String {
    let shown = runtime.elements();
    let lines = shown.iter().filter_map(|e| e.key.as_deref());
    let mut line = lines.filter_map(|key| key.strip_suffix(".row.L1"));
    let pane = line.find(|key| key.starts_with("code-"));
    pane.unwrap_or("code-0").to_owned()
}

/// The middle of the `nth` character that is not a space of line `n` of the code viewer.
fn glyph(runtime: &Runtime<Win>, n: u32, nth: usize) -> Result<(f32, f32), Box<dyn Error>> {
    let text = format!("{}.row.L{n}", code(runtime));
    let from = by_key(&runtime.elements(), &text).ok_or("no line")?.rect[0];
    let glyphs = painted(runtime, KIND_GLYPH, &text)?;
    let mut code = glyphs.into_iter().filter(|g| g[0] >= from - 1.0);
    let g = code.nth(nth).ok_or("no such glyph")?;
    Ok((g[0] + g[2] / 2.0, g[1] + g[3] / 2.0))
}

fn held(runtime: &mut Runtime<Win>, cmd: bool) {
    let mods = Mods {
        cmd,
        ..Mods::default()
    };
    runtime.physical_input(PhysicalInput::Modifiers(mods));
}

fn press_at(runtime: &mut Runtime<Win>, (x, y): (f32, f32)) {
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    rest(runtime);
}

/// The server and the warning the mockup's file has, as the language source writes them.
fn warned() -> Vec<Row> {
    let server = Row::new("server:rust-analyzer")
        .cell("state", Value::Dot(Dot::Run))
        .text("server", "rust-analyzer")
        .text("language", "Rust")
        .text("status", "ready · answered 0 requests");
    let warning = Row::new(format!("problem:{FILE}:20:0"))
        .cell(
            "severity",
            Value::Badge("Warn".into(), yd_core::row::Tone::Warn),
        )
        .text(
            "message",
            "subtracting two Instants can panic if the clock steps back",
        )
        .text("path", FILE)
        .text("line", "20")
        .text("col", "25")
        .text("end_col", "45");
    vec![server, warning]
}

fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/in7-tools")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn stage_of(runtime: &Runtime<Win>, title: &str) -> Result<(String, Stage), Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let all = findings(&hub.model, &ThreadId::new("rate-limit"), Scope::Project);
    let found = all.into_iter().find(|finding| finding.title == title);
    let found = found.ok_or_else(|| format!("no finding {title}"))?;
    Ok((found.id.as_str().to_owned(), found.stage))
}

#[test]
fn the_hub_is_reached_from_the_list_and_a_suggestion_accepted_there_is_kept_on_disk() -> Check {
    let dir = folder("accept")?;
    let mut runtime = unframed("findings/accepting")?;
    start::files(
        &mut *runtime.state.hub.try_borrow_mut()?,
        Files::under(&dir),
    );
    rest(&mut runtime);
    tap(&mut runtime, "findings-0.back")?;
    assert!(!live(&runtime, "findings-0.triage"), "the compact list");
    tap(&mut runtime, "findings-0.hub")?;
    let (id, stage) = stage_of(&runtime, UNLIMITED)?;
    assert_eq!(stage, Stage::New, "it waits in triage");
    tap(
        &mut runtime,
        &format!("findings-0.triage.{id}.finding.accept"),
    )?;
    assert_eq!(stage_of(&runtime, UNLIMITED)?.1, Stage::Open);
    let group = row_text(&runtime.elements(), &format!("findings-0.group.{id}"))?;
    assert!(group.starts_with("Open"), "it is listed as open: {group}");
    start::quit(&mut runtime.state);
    let kept = triage::load(&dir.join("findings.json"));
    assert_eq!(
        kept.decision(&FindingId::new(&id)),
        Some(&Decision::Accepted)
    );
    Ok(())
}

#[test]
fn a_problem_is_a_squiggle_and_resting_on_its_name_asks_for_and_shows_the_card() -> Check {
    let (mut runtime, asks) = tooled("code/mockup")?;
    assert_eq!(painted(&runtime, KIND_SEGMENT, "code-0.L20")?.len(), 0);
    fed(&mut runtime, LSP, warned());
    let wave = painted(&runtime, KIND_SEGMENT, "code-0.L20")?;
    assert!(!wave.is_empty(), "the warning is drawn under line 20");
    let head = row_text(&runtime.elements(), "code-0.breadcrumb")?;
    assert!(head.ends_with("rust-analyzer"), "the file's server: {head}");

    let on_name = glyph(&runtime, 20, 17)?;
    runtime.pointer_move(on_name.0, on_name.1);
    for _ in 0..40 {
        runtime.frame(1.0 / 60.0);
    }
    let hover = format!("lsp lsp.hover yi://lines/{FILE}#L20:C30");
    assert!(asks.borrow().contains(&hover), "{:?}", asks.borrow());
    let said = "The instant the bucket fills again.";
    let told = Row::new(format!("hover:{FILE}:20:30")).text("text", said);
    fed(&mut runtime, LSP, vec![told]);
    let card = row_text(&runtime.elements(), "code-0.hover")?;
    assert!(card.contains(said), "{card}");
    assert!(card.contains("can panic if the clock steps back"), "{card}");

    assert!(!live(&runtime, "code-0.fix-bar"), "no fix is on offer yet");
    let fix = Row::new(format!("fix:{FILE}:20:0")).text("title", "Use saturating_duration_since");
    fed(&mut runtime, LSP, vec![fix]);
    tap(&mut runtime, "code-0.fix-bar.apply")?;
    let apply = format!("lsp lsp.apply-fix yi://problem/fix:{FILE}:20:0");
    assert!(asks.borrow().contains(&apply), "{:?}", asks.borrow());

    runtime.pointer_move(on_name.0, on_name.1);
    rest(&mut runtime);
    tap(&mut runtime, "code-0.hover.link-definition")?;
    let place = Row::new(format!("def:{FILE}:20:30"))
        .text("path", "src/routes/upload.rs")
        .text("line", "9");
    fed(&mut runtime, LSP, vec![place]);
    let hub = runtime.state.hub.try_borrow()?;
    let open = hub.model.tables.get(OPEN_FILES).ok_or("no open files")?;
    let front = (0..open.len()).filter_map(|at| open.get(at));
    let front = front.max_by_key(|tab| yd_core::command::run::opened(tab));
    assert_eq!(
        front.and_then(|tab| tab.words("path")),
        Some("src/routes/upload.rs"),
        "the poll opened where the name is defined"
    );
    Ok(())
}

/// In the grid, where the right-click item finds the window's code viewer.
#[test]
fn a_command_press_on_a_name_lists_its_references_and_attach_all_fills_the_tray() -> Check {
    let (mut runtime, asks) = tooled("shell/everything")?;
    let pane = code(&runtime);
    // The grid's code viewer is ten lines tall: line 13 is brought into it first.
    let (surface, tenth) = (pane.clone(), LineNo::try_from(10)?);
    runtime.update(move |_, tx| reveal(tx, &surface, tenth));
    rest(&mut runtime);
    let on_take = glyph(&runtime, 13, 6)?;
    held(&mut runtime, true);
    press_at(&mut runtime, on_take);
    held(&mut runtime, false);
    let ask = format!("lsp lsp.references yi://lines/{FILE}#L13:C12");
    assert!(asks.borrow().contains(&ask), "{:?}", asks.borrow());
    assert!(
        live(&runtime, &format!("{pane}.refs.head")),
        "the panel waits"
    );
    fed(&mut runtime, LSP, refs::sample());
    let first = format!("{pane}.refs.ref.ref:{FILE}:13:12:0");
    assert!(live(&runtime, &first), "the answer's rows are listed");
    let before = runtime.state.hub.try_borrow()?.model.attached.things.len();
    tap(&mut runtime, &format!("{pane}.refs.attach-all"))?;
    let attached = runtime.state.hub.try_borrow()?.model.attached.things.len();
    assert_eq!(attached - before, 6);

    // The right-click item asks for the name the right-click was on.
    let on_bucket = glyph(&runtime, 12, 5)?;
    runtime.secondary_down(on_bucket.0, on_bucket.1);
    runtime.secondary_up(on_bucket.0, on_bucket.1);
    rest(&mut runtime);
    let line = Address::new(Kind::Lines, FILE)?.part("L12");
    runtime.update(move |win, tx| {
        let find = Run::new(REFERENCES).on(Target::Thing(line.clone()));
        command_box::run(win, tx, find);
    });
    let ask = format!("lsp lsp.references yi://lines/{FILE}#L12:C6");
    assert!(asks.borrow().contains(&ask), "{:?}", asks.borrow());
    Ok(())
}

#[test]
fn the_debuggers_gutter_and_controls_reach_the_debugger_only_when_they_can() -> Check {
    let (mut runtime, asks) = tooled("debugger/mockup")?;
    let shown = runtime.elements();
    let line = by_key(&shown, "debugger-0.source.L16").ok_or("no line 16")?;
    let [x, y, _, h] = line.rect;
    press_at(&mut runtime, (x + 10.0, y + h / 2.0));
    tap(&mut runtime, "debugger-0.control.step-over")?;
    let toggle = format!("debugger debug.break yi://lines/{FILE}#L16");
    assert_eq!(*asks.borrow(), [toggle, "debugger debug.step-over".into()]);

    let running = Row::new("state")
        .text("status", "running")
        .text("program", "api");
    fed(&mut runtime, DEBUGGER, vec![running]);
    tap(&mut runtime, "debugger-0.control.step-over")?;
    assert_eq!(asks.borrow().len(), 2, "a running program took a step");
    Ok(())
}

#[test]
fn the_right_click_rows_of_a_line_and_a_variable_reach_the_debugger() -> Check {
    let (mut runtime, asks) = tooled("debugger/mockup")?;
    let line = Address::new(Kind::Lines, FILE)?.part("L16");
    let variable = Address::new(Kind::Variable, "var:self.tokens")?;
    runtime.update(move |win, tx| {
        let stop = Run::new(BREAK_HERE).on(Target::Thing(line.clone()));
        command_box::run(win, tx, stop);
        // The menu's row, then the Watch button of the variable's opened row.
        for command in [VARIABLE_WATCH, WATCH_VARIABLE] {
            let watch = Run::new(command).on(Target::Thing(variable.clone()));
            command_box::run(win, tx, watch);
        }
    });
    let toggle = format!("debugger debug.break yi://lines/{FILE}#L16");
    let watch = "debugger debug.watch  self.tokens".to_owned();
    assert_eq!(*asks.borrow(), [toggle, watch.clone(), watch]);
    Ok(())
}

#[test]
fn a_servers_log_row_shows_the_logs_pane_listing_that_server_alone() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let server = Address::new(Kind::LanguageServer, "server:rust-analyzer")?;
    runtime.update(move |win, tx| {
        let logs = Run::new(SERVER_LOGS).on(Target::Thing(server.clone()));
        command_box::run(win, tx, logs);
    });
    rest(&mut runtime);
    let shown = runtime.elements();
    let chip = shown.iter().find(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.starts_with("logs-") && key.ends_with(".source")
    });
    let key = chip
        .and_then(|chip| chip.key.clone())
        .ok_or("no logs pane")?;
    // The pane glides in over its neighbour, whose words can lie under the chip.
    let said = row_text(&shown, &key)?;
    assert!(said.starts_with("rust-analyzer"), "{said}");
    Ok(())
}

/// Whether the main window's grid holds a pane of `kind`.
fn docked(runtime: &Runtime<Win>, kind: PaneKind) -> Result<bool, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let window = grid.window(root::MAIN).ok_or("no window")?;
    let mut panes = window.layout.docked().map(|(pane, _)| pane);
    Ok(panes.any(|pane| grid.kind(pane) == Some(kind)))
}

#[test]
fn a_problems_rows_ask_for_its_fix_open_its_line_and_show_its_finding() -> Check {
    let (mut runtime, asks) = tooled("shell/everything")?;
    fed(&mut runtime, LSP, warned());
    let problem = Address::new(Kind::Problem, format!("problem:{FILE}:20:0"))?;
    let server = Address::new(Kind::LanguageServer, "server:rust-analyzer")?;
    assert!(!docked(&runtime, PaneKind::Findings)? && !docked(&runtime, PaneKind::Settings)?);
    runtime.update(move |win, tx| {
        for command in [PROBLEM_FIX, PROBLEM_OPEN, OPEN_FINDING] {
            command_box::run(
                win,
                tx,
                Run::new(command).on(Target::Thing(problem.clone())),
            );
        }
        let settings = Run::new(SETTINGS).on(Target::Thing(server.clone()));
        command_box::run(win, tx, settings);
    });
    rest(&mut runtime);
    let fixes = format!("lsp lsp.fixes yi://lines/{FILE}#L20:C25");
    assert!(asks.borrow().contains(&fixes), "{:?}", asks.borrow());
    assert!(docked(&runtime, PaneKind::Findings)?, "its finding's pane");
    assert!(docked(&runtime, PaneKind::Settings)?, "the settings");
    let hub = runtime.state.hub.try_borrow()?;
    let open = hub.model.tables.get(OPEN_FILES).ok_or("no open files")?;
    let tabs = (0..open.len()).filter_map(|at| open.get(at));
    let front = tabs.max_by_key(|tab| yd_core::command::run::opened(tab));
    assert_eq!(front.and_then(|tab| tab.words("path")), Some(FILE));
    Ok(())
}
