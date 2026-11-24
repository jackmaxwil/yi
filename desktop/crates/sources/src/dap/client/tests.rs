use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedStatus, SourceCmd, Table};
use yd_core::ids::CommandId;
use yd_core::inbox::{Closed, Inbox, inbox};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::row::{Row, Rows, Value};

use super::*;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// The probe's breakpoint line: `left = self->tokens;` in `take`.
const LINE: &str = "L10";

fn here() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dap/client")
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// The scripted adapter replaying `fixtures/<name>.jsonl`.
fn fake(name: &str) -> &'static [Adapter<'static>] {
    let script = leak(here().join("adapter.py").display().to_string());
    let file = leak(
        here()
            .join(format!("fixtures/{name}.jsonl"))
            .display()
            .to_string(),
    );
    let args: &'static [&'static str] = Box::leak(vec![script, "replay", file].into_boxed_slice());
    Box::leak(Box::new([Adapter {
        name: "LLDB",
        id: "lldb-dap",
        locate: Locate::At("/usr/bin/python3"),
        args,
    }]))
}

struct Rig {
    asks: Option<Sender<Ask>>,
    inbox: Inbox,
    model: Model,
    thread: Option<JoinHandle<std::result::Result<(), Closed>>>,
}

impl Rig {
    fn new(adapters: &'static [Adapter<'static>]) -> Self {
        let (sender, inbox) = inbox(|| {});
        let (asks, cmds) = channel();
        let out = sender.feed(FEED);
        let thread = thread::spawn(move || serve(out, cmds, adapters));
        Self {
            asks: Some(asks),
            inbox,
            model: Model::default(),
            thread: Some(thread),
        }
    }

    fn ask(&self, run: Run, from: Origin) -> Result {
        let asks = self.asks.as_ref().ok_or("closed")?;
        asks.send(Ask {
            cmd: SourceCmd::Run(run),
            from,
        })?;
        Ok(())
    }

    fn user(&self, command: CommandId, target: Target, text: &str) -> Result {
        let text = (!text.is_empty()).then(|| text.to_owned());
        self.ask(
            Run {
                text,
                ..Run::new(command).on(target)
            },
            Origin::User,
        )
    }

    fn table(&self) -> Result<&Table> {
        Ok(self.model.tables.get(FEED).ok_or("no debugger table")?)
    }

    fn until(&mut self, what: &str, done: impl Fn(&Table) -> bool) -> Result {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.inbox.drain(&mut self.model);
            if self.model.tables.get(FEED).is_some_and(&done) {
                return Ok(());
            }
            if Instant::now() > deadline {
                let rows = self.model.tables.get(FEED).map(|t| format!("{t:?}"));
                return Err(format!("never saw {what}: {rows:?}").into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Closes the source and waits for its thread, which also ends the adapter.
    fn close(mut self) -> Result {
        self.asks = None;
        let thread = self.thread.take().ok_or("no thread")?;
        thread.join().map_err(|_| "the source panicked")??;
        Ok(())
    }
}

fn rows(table: &Table, prefix: &str) -> Vec<Row> {
    let part = table.prefixed(prefix);
    (0..part.len())
        .filter_map(|n| part.get(n).cloned())
        .collect()
}

fn text(table: &Table, id: &str, field: &str) -> Option<String> {
    let row = table.find(&yd_core::ids::RowId::new(id))?;
    row.words(field).map(str::to_owned)
}

fn flag(table: &Table, id: &str, field: &str) -> Option<bool> {
    match table.find(&yd_core::ids::RowId::new(id))?.get(field)? {
        Value::Flag(on) => Some(*on),
        _ => None,
    }
}

fn is(table: &Table, id: &str, field: &str, want: &str) -> bool {
    text(table, id, field).as_deref() == Some(want)
}

fn probe_file() -> Result<Target> {
    let path = here().join("probe.c").display().to_string();
    Ok(Target::Thing(Address::new(Kind::Lines, path)?.part(LINE)))
}

/// Breakpoint on the probe's line, a watch on `now`, launch, and the first stop arrived.
fn first_stop(rig: &mut Rig, program: &str) -> Result {
    rig.user(BREAK, probe_file()?, "")?;
    rig.user(WATCH, Target::App, "now")?;
    rig.user(LAUNCH, Target::App, program)?;
    rig.until("the first stop", |t| {
        is(t, "state", "status", "paused") && is(t, "watch:1", "value", "0")
    })
}

#[test]
fn a_launch_stops_at_the_breakpoint_with_stack_variables_and_watch() -> Result {
    let mut rig = Rig::new(fake("probe"));
    first_stop(&mut rig, "probe")?;
    let t = rig.table()?;
    assert!(is(t, "state", "adapter", "LLDB"));
    assert!(is(t, "state", "reason", "breakpoint"));
    assert!(is(t, "state", "line", "10"));
    assert!(text(t, "state", "path").is_some_and(|p| p.ends_with("probe.c")));
    assert!(is(t, "frame:0", "function", "take"));
    assert!(flag(t, "frame:0", "current") == Some(true));
    assert!(text(t, "frame:1", "function").is_some_and(|f| f.contains("main")));
    assert!(is(t, "var:left", "value", "0"));
    assert!(is(t, "var:now", "value", "0"));
    assert!(text(t, "var:self", "type").is_some_and(|k| k.contains("bucket")));
    assert!(is(t, "watch:1", "expression", "now"));
    assert!(is(t, "bp:1", "where", "probe.c:10"));
    assert!(flag(t, "bp:1", "enabled") == Some(true));
    assert!(is(t, "bp:1", "hits", "hit 1"));
    rig.close()
}

#[test]
fn an_opened_variable_brings_its_children() -> Result {
    let mut rig = Rig::new(fake("probe"));
    first_stop(&mut rig, "probe")?;
    let self_row = Address::new(Kind::Variable, "var:self")?;
    rig.user(EXPAND, Target::Thing(self_row), "")?;
    rig.until("self's fields", |t| is(t, "var:self.tokens", "value", "3"))?;
    let t = rig.table()?;
    assert!(is(t, "var:self.refill_at", "value", "10"));
    let child = t.find(&yd_core::ids::RowId::new("var:self.tokens"));
    assert_eq!(
        child.and_then(|r| r.parent.as_ref()).map(|p| p.as_str()),
        Some("var:self")
    );
    rig.close()
}

#[test]
fn a_step_marks_the_variable_it_changed_with_what_it_was() -> Result {
    let mut rig = Rig::new(fake("probe"));
    first_stop(&mut rig, "probe")?;
    let self_row = Address::new(Kind::Variable, "var:self")?;
    rig.user(EXPAND, Target::Thing(self_row), "")?;
    rig.until("self's fields", |t| is(t, "var:self.tokens", "value", "3"))?;
    rig.user(STEP_OVER, Target::App, "")?;
    rig.until("the step", |t| is(t, "var:left", "value", "3"))?;
    let t = rig.table()?;
    assert!(is(t, "state", "reason", "step"));
    assert!(is(t, "state", "line", "11"));
    assert_eq!(flag(t, "var:left", "changed"), Some(true));
    assert!(is(t, "var:left", "was", "0"));
    assert_eq!(flag(t, "var:now", "changed"), Some(false));
    rig.close()
}

/// The recording past the step: continue to the second call, then ask the console.
fn second_stop(rig: &mut Rig) -> Result {
    first_stop(rig, "probe")?;
    let self_row = Address::new(Kind::Variable, "var:self")?;
    rig.user(EXPAND, Target::Thing(self_row), "")?;
    rig.until("self's fields", |t| is(t, "var:self.tokens", "value", "3"))?;
    rig.user(STEP_OVER, Target::App, "")?;
    rig.until("the step", |t| is(t, "var:left", "value", "3"))?;
    rig.user(CONTINUE, Target::App, "")?;
    rig.until("the second stop", |t| is(t, "watch:1", "value", "1"))
}

#[test]
fn a_watch_is_evaluated_again_at_every_stop() -> Result {
    let mut rig = Rig::new(fake("probe"));
    second_stop(&mut rig)?;
    let t = rig.table()?;
    assert!(is(t, "watch:1", "was", "0"));
    assert!(is(t, "var:now", "value", "1"));
    assert_eq!(flag(t, "var:now", "changed"), Some(true));
    assert!(is(t, "bp:1", "hits", "hit 2"));
    rig.close()
}

#[test]
fn the_console_shows_what_was_typed_and_its_answer() -> Result {
    let mut rig = Rig::new(fake("probe"));
    second_stop(&mut rig)?;
    rig.user(EVAL, Target::App, "left + 40")?;
    rig.until("the answer", |t| {
        rows(t, "out:")
            .iter()
            .any(|r| r.words("text").is_some_and(|w| w.ends_with("= 40")))
    })?;
    let t = rows(rig.table()?, "out:");
    let typed = t.iter().find(|r| r.words("text") == Some("left + 40"));
    assert_eq!(typed.and_then(|r| r.words("role")), Some("input"));
    rig.close()
}

#[test]
fn a_breakpoint_toggled_off_lets_the_program_run_to_its_exit() -> Result {
    let mut rig = Rig::new(fake("probe"));
    second_stop(&mut rig)?;
    rig.user(EVAL, Target::App, "left + 40")?;
    rig.until("the answer", |t| {
        rows(t, "out:")
            .iter()
            .any(|r| r.words("text").is_some_and(|w| w.ends_with("= 40")))
    })?;
    rig.user(BREAK, probe_file()?, "")?;
    rig.until("the breakpoint gone", |t| rows(t, "bp:").is_empty())?;
    rig.user(CONTINUE, Target::App, "")?;
    rig.until("the exit", |t| is(t, "state", "status", "exited"))?;
    let t = rig.table()?;
    assert_eq!(t.status(), &FeedStatus::Live);
    assert!(
        rows(t, "out:")
            .iter()
            .any(|r| r.words("text") == Some("left 2"))
    );
    rig.close()
}

#[test]
fn an_agent_may_read_a_paused_program_but_not_step_it() -> Result {
    let mut rig = Rig::new(fake("probe"));
    first_stop(&mut rig, "probe")?;
    for command in [CONTINUE, STEP_OVER, STEP_IN, STEP_OUT, RESTART, STOP, EVAL] {
        rig.ask(Run::new(command), Origin::Agent)?;
    }
    rig.ask(Run::new(BREAK).on(probe_file()?), Origin::Agent)?;
    let self_row = Address::new(Kind::Variable, "var:self")?;
    rig.ask(Run::new(EXPAND).on(Target::Thing(self_row)), Origin::Agent)?;
    rig.until("self's fields", |t| is(t, "var:self.tokens", "value", "3"))?;
    let t = rig.table()?;
    let refused = rows(t, "out:");
    let refused = refused
        .iter()
        .filter(|r| r.words("role") == Some("error"))
        .count();
    assert_eq!(refused, 8);
    assert!(is(t, "state", "status", "paused"));
    assert!(rows(t, "bp:").len() == 1);
    rig.user(STEP_OVER, Target::App, "")?;
    rig.until("the user's step", |t| is(t, "var:left", "value", "3"))?;
    rig.close()
}

#[test]
fn a_message_over_the_cap_ends_the_session_cleanly() -> Result {
    let mut rig = Rig::new(fake("too-large"));
    rig.user(LAUNCH, Target::App, "probe")?;
    rig.until("the session's end", |t| is(t, "state", "status", "error"))?;
    let t = rig.table()?;
    assert!(
        text(t, "state", "reason").is_some_and(|r| r.contains("over 16777216")),
        "{:?}",
        text(t, "state", "reason")
    );
    rig.user(LAUNCH, Target::App, "probe")?;
    rig.until("a second start", |t| is(t, "state", "status", "error"))?;
    rig.close()
}

#[test]
fn an_adapter_that_dies_sets_an_error_status() -> Result {
    let mut rig = Rig::new(fake("dies"));
    rig.user(LAUNCH, Target::App, "probe")?;
    rig.until("the error", |t| matches!(t.status(), FeedStatus::Error(_)))?;
    let t = rig.table()?;
    assert!(is(t, "state", "status", "error"));
    assert!(text(t, "state", "reason").is_some_and(|r| r.contains("closed")));
    rig.close()
}

#[test]
fn an_adapter_that_never_answers_initialize_ends_with_an_error() -> Result {
    let mut rig = Rig::new(fake("silent"));
    rig.user(LAUNCH, Target::App, "probe")?;
    rig.until("the error", |t| matches!(t.status(), FeedStatus::Error(_)))?;
    let t = rig.table()?;
    assert!(is(t, "state", "status", "error"));
    let reason = text(t, "state", "reason").unwrap_or_default();
    assert!(reason.contains("did not answer initialize"), "{reason}");
    rig.close()
}

#[test]
fn an_adapter_that_is_not_installed_is_said_in_the_state_row() -> Result {
    let missing: &'static [Adapter<'static>] = &[Adapter {
        name: "GDB",
        id: "gdb",
        locate: Locate::Path("yd-no-such-debugger"),
        args: &[],
    }];
    let mut rig = Rig::new(missing);
    rig.user(LAUNCH, Target::App, "probe")?;
    rig.until("the missing adapter", |t| {
        is(t, "state", "status", "not installed")
    })?;
    let t = rig.table()?;
    assert_eq!(t.status(), &FeedStatus::Live);
    assert!(text(t, "state", "reason").is_some_and(|r| r.contains("yd-no-such-debugger")));
    rig.close()
}

/// Debugs the probe with the real `lldb-dap`, through the recorder, writing
/// `target/dap/probe.jsonl`: the recording `fixtures/probe.jsonl` is that file, paths scrubbed.
#[test]
#[ignore = "needs lldb-dap and clang on this Mac"]
fn the_real_lldb_debugs_the_probe() -> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/dap");
    std::fs::create_dir_all(&root)?;
    let program = root.join("probe");
    #[allow(clippy::disallowed_methods, reason = "builds the probe")]
    let built = std::process::Command::new("/usr/bin/clang")
        .args(["-g", "-O0", "-o"])
        .arg(&program)
        .arg(here().join("probe.c"))
        .status()?;
    assert!(built.success());
    let found = locate(&ADAPTERS[0].locate).ok_or("no lldb-dap")?;
    let script = leak(here().join("adapter.py").display().to_string());
    let log = leak(root.join("probe.jsonl").display().to_string());
    let real = leak(found.display().to_string());
    let args: &'static [&'static str] =
        Box::leak(vec![script, "record", log, real].into_boxed_slice());
    let adapters: &'static [Adapter<'static>] = Box::leak(Box::new([Adapter {
        locate: Locate::At("/usr/bin/python3"),
        args,
        ..ADAPTERS[0]
    }]));
    let mut rig = Rig::new(adapters);
    let program = leak(program.display().to_string());
    first_stop(&mut rig, program)?;
    let self_row = Address::new(Kind::Variable, "var:self")?;
    rig.user(EXPAND, Target::Thing(self_row), "")?;
    rig.until("self's fields", |t| is(t, "var:self.tokens", "value", "3"))?;
    rig.user(STEP_OVER, Target::App, "")?;
    rig.until("the step", |t| is(t, "var:left", "value", "3"))?;
    rig.user(CONTINUE, Target::App, "")?;
    rig.until("the second stop", |t| is(t, "watch:1", "value", "1"))?;
    rig.user(EVAL, Target::App, "left + 40")?;
    rig.until("the answer", |t| {
        rows(t, "out:")
            .iter()
            .any(|r| r.words("text").is_some_and(|w| w.ends_with("= 40")))
    })?;
    rig.user(BREAK, probe_file()?, "")?;
    rig.until("the breakpoint gone", |t| rows(t, "bp:").is_empty())?;
    rig.user(CONTINUE, Target::App, "")?;
    rig.until("the exit", |t| is(t, "state", "status", "exited"))?;
    rig.close()
}
