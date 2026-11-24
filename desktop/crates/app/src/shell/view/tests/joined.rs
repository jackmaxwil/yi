//! Wave 6 joined to the running window: what the buttons of an opened row do.

use yd_core::address::uri::Address;
use yd_core::command::by_kind::FINDING;
use yd_core::ids::RowId;
use yd_core::kind::Kind;
use yd_core::pane::PaneKind;
use yd_ui::list::state::Lists;
use yd_ui::menu::command_box;

use super::*;

/// The words the window shows now.
fn words(runtime: &Runtime<Win>) -> Vec<String> {
    let shown = runtime.elements();
    let live = shown.into_iter().filter(|element| !element.exiting);
    live.filter_map(|element| element.text).collect()
}

fn says(runtime: &Runtime<Win>, part: &str) -> bool {
    words(runtime).iter().any(|said| said.contains(part))
}

/// Every request line a window sent.
type Sent = Rc<RefCell<Vec<String>>>;

/// The scene `name` at rest, with every request the window sends from here on.
fn watched(name: &str) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let mut runtime = unframed(name)?;
    rest(&mut runtime);
    let lines = sent(&runtime)?;
    Ok((runtime, lines))
}

fn answered(lines: &Sent, ask: &str, option: &str) -> bool {
    let lines = lines.borrow();
    let mut answers = lines.iter().filter(|line| line.contains(ask));
    answers.any(|line| line.contains(&format!("\"optionId\":\"{option}\"")))
}

/// The made findings, as the evidence each was made from.
fn made(runtime: &Runtime<Win>) -> Result<Vec<String>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let made = hub.model.findings.triage.made.iter();
    Ok(made.map(|made| made.evidence.to_string()).collect())
}

#[test]
fn run_its_command_answers_the_ask_the_agent_waits_on() -> Check {
    let (mut runtime, lines) = watched("agents/live")?;
    tap(&mut runtime, "agents-0.row.tests")?;
    tap(&mut runtime, "agents-0.detail.tests.agents.run-command")?;
    assert!(
        answered(&lines, "\"id\":\"perm-1\"", "allow_once"),
        "the ask was not allowed: {:?}",
        lines.borrow()
    );
    Ok(())
}

#[test]
fn open_thread_shows_the_childs_own_session() -> Check {
    let (mut runtime, lines) = watched("agents/live")?;
    tap(&mut runtime, "agents-0.row.fork")?;
    tap(&mut runtime, "agents-0.detail.fork.agents.open-thread")?;
    let hub = runtime.state.hub.try_borrow()?;
    let shown = yd_ui::shown::thread(&hub, root::MAIN).map(|thread| thread.id.to_string());
    assert_eq!(
        shown.as_deref(),
        Some("fork-session"),
        "{:?}",
        lines.borrow()
    );
    Ok(())
}

#[test]
fn a_button_nothing_carries_says_so_when_it_is_pressed_and_not_before() -> Check {
    let (mut runtime, lines) = watched("agents/live")?;
    tap(&mut runtime, "agents-0.row.tests")?;
    assert!(
        !says(&runtime, "not available yet"),
        "a row says so unasked"
    );
    tap(&mut runtime, "agents-0.detail.tests.agents.pause")?;
    assert!(
        says(&runtime, "Pause is not available yet"),
        "{:?}",
        words(&runtime)
    );
    assert!(lines.borrow().is_empty(), "and nothing was sent");
    Ok(())
}

#[test]
fn run_it_on_a_waiting_step_allows_the_ask_that_gates_it() -> Check {
    let (mut runtime, lines) = watched("trace/live")?;
    tap(&mut runtime, "trace-0.detail.wait-c1.trace.run")?;
    assert!(
        answered(&lines, "\"id\":\"perm_", "allow_once"),
        "{:?}",
        lines.borrow()
    );
    Ok(())
}

#[test]
fn deny_on_a_waiting_step_rejects_the_ask_that_gates_it() -> Check {
    let (mut runtime, lines) = watched("trace/live")?;
    tap(&mut runtime, "trace-0.detail.wait-c1.trace.deny")?;
    assert!(
        answered(&lines, "\"id\":\"perm_", "reject_once"),
        "{:?}",
        lines.borrow()
    );
    Ok(())
}

#[test]
fn attach_on_a_step_puts_the_step_in_the_tray() -> Check {
    let (mut runtime, _) = watched("trace/live")?;
    tap(&mut runtime, "trace-0.detail.wait-c1.trace.attach")?;
    let hub = runtime.state.hub.try_borrow()?;
    let held: Vec<String> = hub.model.attached.things.iter().map(address).collect();
    assert_eq!(held, ["yi://span/wait-c1"]);
    Ok(())
}

fn address(thing: &yd_core::address::thing::Thing) -> String {
    thing.address.to_string()
}

const ERROR_LINE: &str = "logs-0.detail.17:58:03.412";

#[test]
fn the_only_button_names_the_lines_source_and_lists_that_source_alone() -> Check {
    let (mut runtime, _) = watched("logs/mockup")?;
    let button = format!("{ERROR_LINE}.logs.only-source");
    assert_eq!(row_text(&runtime.elements(), &button)?, "Only docker:db");
    tap(&mut runtime, &button)?;
    assert!(live(&runtime, "logs-0.row.17:58:03.412"), "its own line");
    assert!(
        !live(&runtime, "logs-0.row.17:58:03.050"),
        "a line of another source is still listed"
    );
    Ok(())
}

#[test]
fn make_it_a_finding_on_a_log_line_makes_one_from_that_line() -> Check {
    let (mut runtime, _) = watched("logs/mockup")?;
    tap(&mut runtime, &format!("{ERROR_LINE}.logs.make-finding"))?;
    assert_eq!(made(&runtime)?, ["yi://log-line/17:58:03.412"]);
    Ok(())
}

#[test]
fn the_right_click_row_makes_a_finding_of_the_thing_it_is_on() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let file = Address::new(Kind::File, "src/routes/upload.rs")?;
    let target = Target::Thing(file);
    runtime.update(move |win, tx| {
        command_box::run(win, tx, Run::new(FINDING).on(target.clone()));
    });
    assert_eq!(made(&runtime)?, ["yi://file/src/routes/upload.rs"]);
    Ok(())
}

/// A line the model gave for a tool call names its step: the Trace pane opens on it.
#[test]
fn show_in_trace_opens_the_trace_pane_on_the_lines_step() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let line = {
        let hub = runtime.state.hub.try_borrow()?;
        let thread = yd_ui::shown::thread(&hub, root::MAIN);
        let lines = yd_panes::logs::pane::lines(&hub.model, thread);
        let call = lines.into_iter().find(|row| row.words("span").is_some());
        call.ok_or("no line of a tool call")?
    };
    let step = line.words("span").ok_or("no step")?.to_owned();
    let target = Target::Thing(Address::new(Kind::LogLine, line.id.as_str())?);
    runtime.update(move |win, tx| {
        let show = Run::new(yd_panes::logs::desc::SHOW_IN_TRACE).on(target.clone());
        command_box::run(win, tx, show);
    });
    rest(&mut runtime);
    let (trace, open) = {
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let window = grid.window(root::MAIN).ok_or("no window")?;
        let mut docked = window.layout.docked().map(|(pane, _)| pane);
        let trace = docked
            .find(|pane| grid.kind(*pane) == Some(PaneKind::Trace))
            .ok_or("no trace pane in the window")?;
        let lists = hub.panes.get::<Lists>(Some(trace)).ok_or("no list")?;
        (trace, lists.list("").and_then(|list| list.open()).cloned())
    };
    assert_eq!(open, Some(RowId::new(step)));
    assert!(
        live(&runtime, &format!("trace-{}.pane-frame", trace.0)),
        "the pane is drawn"
    );
    Ok(())
}

/// The group's own tab stays; All and Changed go back to it. Only those two carry a cross.
#[test]
fn settings_draws_a_close_cross_only_on_the_tabs_that_close() -> Check {
    let runtime = runtime("settings/mockup")?;
    let cross = |tab: u32| live(&runtime, &format!("settings-0.tab-{tab}.pane-tab-close"));
    assert_eq!([cross(0), cross(1), cross(2)], [false, true, true]);
    Ok(())
}
