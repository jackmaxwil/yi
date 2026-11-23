//! What someone debugging would notice if the pane broke, drawn in process with no GPU.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::debug;
use yd_core::feed::{DEBUGGER, FILES, FeedEvent, SourceCmd};
use yd_core::ids::{RowId, WindowId};
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Tone, Value};
use yd_ui::grid_view::view::alone;
use yd_ui::pane::PaneDef;
use yd_ui::win::{Hub, Win};

use super::session::{Control, Session, Status};
use super::{DEF, scenes};
use crate::debugger::desc::{self, variable_row};

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;
type Sent = Rc<RefCell<Vec<Effect>>>;

static DEFS: [&PaneDef; 1] = [&DEF];
const PANE: &str = "debugger-0";

fn root(win: &Win, cx: &mut rune::Cx<Win>) -> rune::El {
    let Ok(hub) = win.hub.try_borrow() else {
        return rune::col();
    };
    alone(win, &hub, cx, PaneKind::Debugger)
}

fn scene(name: &str) -> Result<(Window, Sent), Box<dyn Error>> {
    let scene = scenes::SCENES
        .iter()
        .find(|scene| scene.name == name)
        .ok_or_else(|| format!("no scene {name}"))?;
    let hub = Hub::new(&DEFS, Vec::new());
    let sent = Sent::default();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = scene.stage;
        (scene.setup)(&mut hub);
        let log = sent.clone();
        hub.set_sink(move |effect| log.borrow_mut().push(effect));
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (629.0, 700.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    Ok((rt, sent))
}

fn settle(rt: &mut Window) {
    for _ in 0..12 {
        rt.frame(1.0 / 60.0);
    }
}

fn find(rt: &Window, key: &str) -> Result<ElementInfo, String> {
    let live = rt.elements().into_iter().filter(|e| !e.exiting);
    live.into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .ok_or_else(|| format!("no element keyed {key}"))
}

/// The words in the keyed element, left to right, one space apart.
fn words(rt: &Window, key: &str) -> Result<String, String> {
    let [x, y, w, h] = find(rt, key)?.rect;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let mut found: Vec<(f32, String)> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && inside(&e.rect))
        .filter_map(|e| Some((e.rect[0], e.text.filter(|t| !t.is_empty())?)))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let joined = found
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join(" ");
    Ok(joined.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The words in the keyed element, top to bottom and then left to right.
fn stacked(rt: &Window, key: &str) -> Result<String, String> {
    let [x, y, w, h] = find(rt, key)?.rect;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let mut found: Vec<([f32; 2], String)> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && inside(&e.rect))
        .filter_map(|e| {
            Some((
                [(e.rect[1] / 8.0).floor(), e.rect[0]],
                e.text.filter(|t| !t.is_empty())?,
            ))
        })
        .collect();
    found.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then(a.0[1].total_cmp(&b.0[1])));
    let joined = found
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join(" ");
    Ok(joined.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn tap_at(rt: &mut Window, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn tap(rt: &mut Window, key: &str) -> Check {
    let [x, y, w, h] = find(rt, key)?.rect;
    tap_at(rt, (x + w / 2.0, y + h / 2.0));
    Ok(())
}

fn press(rt: &mut Window, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
}

/// The runs the pane has sent to the debugger source.
fn runs(sent: &Sent) -> Vec<Run> {
    let log = sent.borrow();
    let runs = log.iter().filter_map(|effect| match effect {
        Effect::Source(feed, SourceCmd::Run(run)) if *feed == DEBUGGER => Some(run.clone()),
        _ => None,
    });
    runs.collect()
}

/// Folds events into the debugger feed as its source would, and lets the pane see them.
fn feed(rt: &mut Window, events: Vec<FeedEvent>) {
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            for event in events.clone() {
                step(&mut hub.model, Msg::Feed(DEBUGGER, event));
            }
        }
    });
    settle(rt);
}

#[test]
fn the_paused_stop_shows_its_header_lists_console_and_the_rule() -> Check {
    let (rt, _) = scene("debugger/mockup")?;
    assert_eq!(
        words(&rt, &format!("{PANE}.pane-header"))?,
        "LLDB GDB api debug build · pid 48213 · paused at breakpoint 2 ▶ ↷ ↓ ↑ ⟲ ■"
    );
    assert_eq!(
        words(&rt, &format!("{PANE}.footer"))?,
        "The agent can read this state. It cannot step."
    );
    assert_eq!(
        words(&rt, &format!("{PANE}.variables.row.var:self.refill_at"))?,
        "refill_at 1772.62s Instant"
    );
    assert_eq!(
        stacked(&rt, &format!("{PANE}.variables.detail.var:self.tokens"))?,
        "Type u32 Now 0 Was 1, one stop ago Changed at rate_limit.rs:19 \
         Stop when it changes Watch Attach"
    );
    assert_eq!(
        words(&rt, &format!("{PANE}.stack.row.frame:3"))?,
        "#3 axum::serve library"
    );
    assert_eq!(
        words(&rt, &format!("{PANE}.breakpoints.row.bp:3"))?,
        "○ upload.rs:31 off"
    );
    assert_eq!(
        stacked(&rt, &format!("{PANE}.console"))?,
        "(lldb) breakpoint set -f rate_limit.rs -l 18 -c 'self.tokens == 0' Breakpoint 2: \
         where = api`Bucket::take + 96 at rate_limit.rs:18 (lldb) p self.refill_at - now \
         (core::time::Duration) 41.2s (lldb)"
    );
    Ok(())
}

#[test]
fn pressing_step_over_sends_the_command_and_a_running_program_cannot_be_stepped() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    tap(&mut rt, &format!("{PANE}.control.step-over"))?;
    let step = runs(&sent);
    assert_eq!(step.len(), 1, "one press, one command: {step:?}");
    assert_eq!(step[0].command, debug::STEP_OVER);

    let running = Row::new("state")
        .text("status", "running")
        .text("program", "api");
    feed(&mut rt, vec![FeedEvent::Upsert(running)]);
    tap(&mut rt, &format!("{PANE}.control.step-over"))?;
    tap(&mut rt, &format!("{PANE}.control.continue"))?;
    assert_eq!(runs(&sent).len(), 1, "a running program took a step");
    tap(&mut rt, &format!("{PANE}.control.stop"))?;
    assert_eq!(runs(&sent).last().map(|run| run.command), Some(debug::STOP));
    Ok(())
}

#[test]
fn the_values_of_the_variables_on_the_stop_line_show_after_it() -> Check {
    let (rt, _) = scene("debugger/mockup")?;
    let ends = |line: u32, hint: &str| -> Result<bool, String> {
        Ok(words(&rt, &format!("{PANE}.source.L{line}.value"))?.ends_with(hint))
    };
    assert!(
        ends(18, "self.tokens = 0")?,
        "the stop line shows its variable"
    );
    assert!(
        ends(13, "now = 1731.42s")?,
        "the function's line shows its argument"
    );
    assert!(
        find(&rt, &format!("{PANE}.source.L14.value")).is_err(),
        "a line the stop is not on or in"
    );
    Ok(())
}

#[test]
fn pressing_the_gutter_toggles_a_breakpoint_on_that_line() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    for (line, shown) in [(16, false), (14, true)] {
        let key = format!("{PANE}.source.L{line}");
        assert_eq!(
            find(&rt, &format!("{PANE}.source.L{line}.breakpoint")).is_ok(),
            shown,
            "line {line} starts {}",
            if shown { "marked" } else { "unmarked" }
        );
        let [x, y, _, h] = find(&rt, &key)?.rect;
        tap_at(&mut rt, (x + 10.0, y + h / 2.0));
    }
    let toggled: Vec<String> = runs(&sent)
        .iter()
        .map(|run| match &run.target {
            Target::Thing(at) => format!("{} {at}", run.command.0),
            other => format!("{} {other:?}", run.command.0),
        })
        .collect();
    let file = "yi://lines/src/middleware/rate_limit.rs";
    assert_eq!(
        toggled,
        [
            format!("debug.break {file}#L16"),
            format!("debug.break {file}#L14")
        ]
    );
    Ok(())
}

/// The colour of the words `said` inside the keyed row.
fn ink(rt: &Window, row: &str, said: &str) -> Result<rune::Color, String> {
    let [x, y, w, h] = find(rt, row)?.rect;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let words = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && inside(&e.rect));
    let found = words.into_iter().find(|e| e.text.as_deref() == Some(said));
    found
        .and_then(|e| e.text_color)
        .ok_or_else(|| format!("no {said:?} in {row}"))
}

#[test]
fn a_changed_variable_is_marked_in_the_need_colour_with_what_it_was() -> Check {
    let (rt, _) = scene("debugger/mockup")?;
    let need = {
        let hub = rt.state.hub.try_borrow()?;
        yd_ui::theme::look::colour(hub.model.tokens.need)
    };
    let changed = ink(&rt, &format!("{PANE}.variables.row.var:self.tokens"), "0")?;
    let steady = ink(
        &rt,
        &format!("{PANE}.variables.row.var:self.refill_at"),
        "1772.62s",
    )?;
    assert_eq!(changed, need, "tokens changed from 1 to 0");
    assert_ne!(steady, need, "refill_at did not change");

    let row = Row::new("var:n")
        .text("name", "n")
        .text("type", "u32")
        .text("was", "1");
    let row = variable_row(row.cell("changed", Value::Flag(true)));
    assert_eq!(
        (row.tone, row.words("shown_type")),
        (Some(Tone::Need), Some("u32 · was 1"))
    );
    Ok(())
}

#[test]
fn pressing_a_frame_shows_that_frames_source() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    assert_eq!(
        words(&rt, &format!("{PANE}.pane-tab-bar"))?,
        "Debugger rate_limit.rs pid 48213 · 5 threads"
    );
    tap(&mut rt, &format!("{PANE}.stack.row.frame:2"))?;
    assert_eq!(
        words(&rt, &format!("{PANE}.source.state"))?,
        "Reading src/routes/upload.rs"
    );
    let asked = sent.borrow().iter().any(|effect| {
        matches!(effect, Effect::Source(feed, SourceCmd::Read(path)) if *feed == FILES && path.ends_with("upload.rs"))
    });
    assert!(asked, "the files source was not asked for upload.rs");
    assert_eq!(
        words(&rt, &format!("{PANE}.pane-tab-bar"))?,
        "Debugger upload.rs pid 48213 · 5 threads"
    );

    let read = Row::new("file:src/routes/upload.rs")
        .text("path", "src/routes/upload.rs")
        .text("text", "use axum::Router;\n\nfn upload() {}\n");
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(
                &mut hub.model,
                Msg::Feed(FILES, FeedEvent::Upsert(read.clone())),
            );
        }
    });
    settle(&mut rt);
    assert_eq!(
        words(&rt, &format!("{PANE}.source.L3"))?,
        "3 fn upload() {}"
    );
    Ok(())
}

#[test]
fn what_is_typed_in_the_console_is_sent_to_the_debugger() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    tap(&mut rt, &format!("{PANE}.console.input"))?;
    press(&mut rt, Key::Text("p self.tokens".into()));
    press(&mut rt, Key::Enter);
    let eval = runs(&sent);
    assert_eq!(eval.len(), 1, "{eval:?}");
    assert_eq!(
        (eval[0].command, eval[0].text.as_deref()),
        (debug::EVAL, Some("p self.tokens"))
    );
    Ok(())
}

#[test]
fn a_watch_is_added_by_typing_it() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    tap(&mut rt, &format!("{PANE}.watch.add.input"))?;
    press(&mut rt, Key::Text("self.tokens + 1".into()));
    press(&mut rt, Key::Enter);
    let added = runs(&sent);
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(
        (added[0].command, added[0].text.as_deref()),
        (debug::WATCH, Some("self.tokens + 1"))
    );
    Ok(())
}

#[test]
fn opening_a_variable_with_fields_asks_the_debugger_for_them() -> Check {
    let (mut rt, sent) = scene("debugger/mockup")?;
    // The source sends `self` before its fields, and a stop with none fetched yet.
    let fields = ["var:self.tokens", "var:self.refill_at"];
    feed(
        &mut rt,
        fields
            .iter()
            .map(|id| FeedEvent::Remove(RowId::new(*id)))
            .collect(),
    );
    tap(&mut rt, &format!("{PANE}.variables.row.var:self"))?;
    let asked = runs(&sent);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let aimed = match &asked[0].target {
        Target::Thing(at) => at.to_string(),
        other => format!("{other:?}"),
    };
    assert_eq!(
        (asked[0].command, aimed.as_str()),
        (debug::EXPAND, "yi://variable/var:self")
    );
    Ok(())
}

#[test]
fn nothing_started_is_said_in_words_and_no_control_works() -> Check {
    let (mut rt, sent) = scene("debugger/not-started")?;
    assert_eq!(
        stacked(&rt, &format!("{PANE}.source.state"))?,
        "Nothing is being debugged Launch a program from the command box: Launch in Debugger."
    );
    for control in [
        "continue",
        "step-over",
        "step-in",
        "step-out",
        "restart",
        "stop",
    ] {
        tap(&mut rt, &format!("{PANE}.control.{control}"))?;
    }
    assert!(
        runs(&sent).is_empty(),
        "a control worked with nothing started"
    );
    Ok(())
}

#[test]
fn a_missing_debugger_and_an_exited_program_say_so() {
    let word = |status: &str, reason: &str| {
        let row = Row::new("state")
            .text("status", status)
            .text("reason", reason);
        crate::debugger::source::plain::words(Some(&Session::from(&row))).0
    };
    assert_eq!(word("not installed", ""), "No debugger found");
    assert_eq!(word("exited", "exit code 0"), "The program exited");
    assert_eq!(word("error", "adapter closed"), "The debugger stopped");
}

#[test]
fn only_a_paused_program_can_be_stepped_and_only_a_started_one_restarted() {
    let state = |status: &str, program: &str| {
        Session::from(
            &Row::new("state")
                .text("status", status)
                .text("program", program),
        )
    };
    let can = |session: &Session| -> Vec<bool> {
        [
            Control::Continue,
            Control::StepOver,
            Control::Restart,
            Control::Stop,
        ]
        .into_iter()
        .map(|control| session.can(control))
        .collect()
    };
    assert_eq!(can(&state("paused", "api")), [true, true, true, true]);
    assert_eq!(can(&state("running", "api")), [false, false, true, true]);
    assert_eq!(can(&state("exited", "api")), [false, false, true, false]);
    assert_eq!(can(&state("idle", "")), [false, false, false, false]);
    assert_eq!(
        state("something new", "").status,
        Status::Other("something new".into())
    );
    assert_eq!(
        desc::of_row("watch:1").map(|d| d.kind),
        Some(yd_core::kind::Kind::Variable)
    );
}
