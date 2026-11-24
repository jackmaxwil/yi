//! What a person reading the logs would notice if it broke, drawn in process with no GPU.

use std::error::Error;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::feed::{FeedEvent, Table};
use yd_core::ids::{RowId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::row::{Row, Rows, rows_from_json};
use yd_ui::list::state::Lists;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::shown::{WINDOW, shown};
use super::state::{Filters, Level, State};
use super::{DEF, FEED, carry, scenes, thing_at};
use crate::logs::desc::{ONLY_SOURCE, SHOW_IN_TRACE};

type Check = Result<(), Box<dyn Error>>;

const MOCKUP: &str = include_str!("../../../../../fixtures/mockup/logs.rows.json");

fn fixture() -> Result<Vec<Row>, Box<dyn Error>> {
    Ok(rows_from_json(MOCKUP)?)
}

fn table(rows: Vec<Row>) -> Table {
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(rows));
    table
}

fn ids(shown: &super::shown::Shown) -> Vec<String> {
    let at = |n| shown.table.get(n).map(|row| row.id.as_str().to_owned());
    (0..shown.table.len()).filter_map(at).collect()
}

/// A line as the source writes it: a time, a level, a source, words and the raw record.
fn line(n: u64, level: &str, source: &str, said: &str) -> Row {
    Row::new(format!("{n}-{n}"))
        .text("ts", (1_000_000_000_000 + n).to_string())
        .text("time", format!("00:00:{:02}.000", n % 60))
        .text("level", level)
        .text("source", source)
        .text("message", said)
        .text("json", said)
}

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

fn draw(setup: fn(&mut Hub)) -> Result<Runtime<Win>, Box<dyn Error>> {
    drawn_in(setup, (700.0, 420.0))
}

fn drawn_in(setup: fn(&mut Hub), size: (f32, f32)) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), root, size, Env::new(true, 1.0));
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok(rt)
}

#[test]
fn a_narrow_logs_pane_drops_its_time_and_source_and_gives_the_message_most_of_the_width() -> Check {
    let (wide, narrow) = (draw(plain)?, drawn_in(plain, (270.0, 420.0))?);
    let at = |rt: &Runtime<Win>, words: &str| {
        let all = rt.elements();
        let hit = all
            .iter()
            .find(|e| !e.exiting && e.text.as_deref() == Some(words));
        hit.map(|e| e.rect)
    };
    assert!(
        at(&wide, "17:57:58.101").is_some(),
        "a wide pane lists the time"
    );
    assert!(
        at(&narrow, "17:57:58.101").is_none(),
        "the time kept its column"
    );
    let message = at(&narrow, "turn 2 started · thread rate-limit").ok_or("no message")?;
    assert!(
        message[2] >= 135.0,
        "the message column is {} wide",
        message[2]
    );
    Ok(())
}

#[test]
fn the_header_counts_the_lines_it_lists_when_the_feed_itself_is_empty() -> Check {
    let setup = crate::chat::DEF
        .scenes
        .iter()
        .find(|scene| scene.name == "chat/mockup")
        .ok_or("no chat/mockup scene")?
        .setup;
    let hub = Hub::new(&[], Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    step(
        &mut hub.try_borrow_mut()?.model,
        Msg::Feed(FEED, FeedEvent::Status(yd_core::feed::FeedStatus::Empty)),
    );
    let hub = hub.try_borrow()?;
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    };
    let (listed, _) = super::made(&pane);
    assert!(listed.total > 1, "the thread lists no lines");
    let meta = super::tabs(&pane)
        .tabs
        .first()
        .map(|tab| tab.meta.clone())
        .ok_or("no tab")?;
    assert_eq!(meta, format!("1 source · {} lines", listed.total));
    Ok(())
}

fn plain(hub: &mut Hub) {
    let rows = fixture().unwrap_or_default();
    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Reset(rows)));
}

fn has_key(rt: &Runtime<Win>, key: &str) -> bool {
    let live = rt.elements();
    live.iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Check {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .ok_or(format!("no {key}"))?;
    let [x, y, w, h] = hit.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    Ok(())
}

fn said(rt: &Runtime<Win>, words: &str) -> bool {
    let live = rt.elements();
    live.iter()
        .any(|e| !e.exiting && e.text.as_deref() == Some(words))
}

#[test]
fn a_level_that_is_off_hides_its_lines_and_turning_it_on_lists_them() -> Check {
    let rows = fixture()?;
    let feed = table(rows);
    let off = shown(Some(&feed), &[], &Filters::default(), "");
    assert_eq!(
        off.table.len(),
        11,
        "the four DEBUG lines of fifteen are hidden"
    );
    assert!(!ids(&off).iter().any(|id| id == "17:58:04.000"));
    let mut on = Filters::default();
    on.flip(Level::Debug);
    assert_eq!(shown(Some(&feed), &[], &on, "").table.len(), 15);
    let mut errors = Filters {
        levels: [true, false, false, false],
        ..Filters::default()
    };
    errors.source = Some("docker:db".to_owned());
    assert_eq!(ids(&shown(Some(&feed), &[], &errors, "")), ["17:58:03.412"]);
    Ok(())
}

#[test]
fn the_header_toggle_hides_the_debug_lines_it_draws_and_lists_them_again() -> Check {
    let mut rt = draw(plain)?;
    assert!(
        has_key(&rt, "logs-0.row.17:58:03.412"),
        "an error line is listed"
    );
    assert!(
        !has_key(&rt, "logs-0.row.17:58:04.000"),
        "Debug is off, yet a DEBUG line is listed"
    );
    press(&mut rt, "logs-0.level.debug")?;
    assert!(
        has_key(&rt, "logs-0.row.17:58:04.000"),
        "Debug is on and its line is hidden"
    );
    press(&mut rt, "logs-0.level.error")?;
    assert!(
        !has_key(&rt, "logs-0.row.17:58:03.412"),
        "Error is off, yet its line is listed"
    );
    Ok(())
}

#[test]
fn a_text_filter_keeps_the_lines_that_name_it_whatever_its_case() -> Check {
    let feed = table(fixture()?);
    let found = shown(Some(&feed), &[], &Filters::default(), "  RETRY-AFTER ");
    assert_eq!(ids(&found), ["17:58:03.007", "17:58:04.512"]);
    let none = shown(Some(&feed), &[], &Filters::default(), "no such words");
    assert_eq!((none.table.len(), none.total), (0, 15));
    Ok(())
}

/// The source's table keeps up to 200,000 lines; Rune re-reads every key of a list when its
/// count changes, so the list holds a window and says what it left out.
#[test]
fn only_the_newest_window_is_listed_and_a_row_names_the_cut() -> Check {
    let all: Vec<Row> = (0..WINDOW as u64 + 10)
        .map(|n| line(n, "INFO", "yi", &format!("line {n}")))
        .collect();
    let feed = table(all);
    let first = shown(Some(&feed), &[], &Filters::default(), "");
    assert_eq!(
        first.table.len(),
        WINDOW + 1,
        "a window and the row that names the cut"
    );
    let cut = first.table.get(0).ok_or("no cut row")?;
    let notice = cut.words("message").unwrap_or("");
    assert!(
        notice.starts_with("[…] 10 older lines are not listed"),
        "{notice}"
    );
    assert!(
        notice.contains("WINDOW 5000"),
        "the notice names its cap: {notice}"
    );
    let newest = first.table.get(WINDOW).ok_or("no newest")?;
    assert_eq!(
        newest.words("message"),
        Some(format!("line {}", WINDOW + 9).as_str())
    );
    let more = Filters {
        older: 1,
        ..Filters::default()
    };
    let second = shown(Some(&feed), &[], &more, "");
    assert_eq!((second.table.len(), second.older), (WINDOW + 10, 0));
    Ok(())
}

#[test]
fn the_models_lines_join_the_feeds_in_time_order_and_stay_open_by_id() -> Check {
    let feed = table(vec![
        line(10, "INFO", "yi", "first"),
        line(30, "INFO", "yi", "third"),
    ]);
    let model = [line(20, "WARN", "agent", "second")];
    let mut wire = model.to_vec();
    wire.iter_mut()
        .for_each(|row| row.id = RowId::new("wire-call-20-0"));
    let both = shown(Some(&feed), &wire, &Filters::default(), "");
    let words: Vec<&str> = (0..both.table.len())
        .filter_map(|n| both.table.get(n)?.words("message"))
        .collect();
    assert_eq!(words, ["first", "second", "third"]);
    assert!(ids(&both).contains(&"wire-call-20-0".to_owned()));
    Ok(())
}

#[test]
fn an_open_line_stops_following_and_the_pill_goes_back_to_the_newest() -> Check {
    let mut rt = draw(scenes::SCENES[0].setup)?;
    assert!(
        said(&rt, "paused"),
        "a line is open, so the list is not following"
    );
    assert!(has_key(&rt, "logs-0.latest"));
    press(&mut rt, "logs-0.latest")?;
    let hub = rt.state.hub.try_borrow()?;
    let open = hub
        .panes
        .get::<Lists>(Some(SCENE_PANE))
        .and_then(|l| l.list("")?.open());
    assert_eq!(open, None, "the pill left the line open");
    drop(hub);
    assert!(
        !has_key(&rt, "logs-0.latest"),
        "the pill stays once the list is back"
    );
    assert!(said(&rt, "following"));
    Ok(())
}

#[test]
fn typing_in_the_filter_narrows_the_list() -> Check {
    let mut rt = draw(plain)?;
    press(&mut rt, "logs-0.filter.input")?;
    rt.key(KeyEvent {
        key: Key::Text("approval".into()),
        mods: Mods::default(),
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert!(
        has_key(&rt, "logs-0.row.17:58:03.050"),
        "the line that names it is gone"
    );
    assert!(
        !has_key(&rt, "logs-0.row.17:58:03.412"),
        "a line that does not is listed"
    );
    Ok(())
}

fn on_line(id: &str) -> Result<Target, Box<dyn Error>> {
    Ok(Target::Thing(Address::new(Kind::LogLine, id)?))
}

#[test]
fn only_this_source_narrows_the_list_to_the_line_s_source() -> Check {
    let mut rt = draw(plain)?;
    let run = Run::new(ONLY_SOURCE).on(on_line("17:58:03.412")?);
    let mut carried = false;
    rt.update(|win, tx| carried = carry(win, tx, run.command, &run.target));
    assert!(carried);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert!(
        has_key(&rt, "logs-0.row.17:58:03.655"),
        "a docker:db line is gone"
    );
    assert!(
        !has_key(&rt, "logs-0.row.17:58:03.050"),
        "a yi line is still listed"
    );
    let state = rt.state.hub.try_borrow()?;
    let kept = state
        .panes
        .get::<State>(Some(SCENE_PANE))
        .map(|s| s.filters.source.clone());
    assert_eq!(kept, Some(Some("docker:db".to_owned())));
    let trace = Run::new(SHOW_IN_TRACE).on(on_line("17:58:03.412")?);
    drop(state);
    rt.update(|win, tx| carried = carry(win, tx, trace.command, &trace.target));
    assert!(
        !carried,
        "a pane drawn alone has no window to open Trace in"
    );
    Ok(())
}

#[test]
fn what_a_key_stands_for_is_found_for_a_line_the_feed_holds() -> Check {
    let rt = draw(plain)?;
    let hub = rt.state.hub.try_borrow()?;
    let thing = thing_at(&hub, SCENE_PANE, "logs-0.row.17:58:03.412").ok_or("no thing")?;
    assert_eq!(thing.address.to_string(), "yi://log-line/17:58:03.412");
    assert!(thing_at(&hub, SCENE_PANE, "logs-0.pane-header").is_none());
    Ok(())
}

/// The model's own lines, from the recorded conversation the chat pane's mockup plays: what the
/// agent ran, in the order it ran it, each tagged `agent`.
#[test]
fn the_agents_tool_calls_become_lines_from_the_model() -> Check {
    let setup = crate::chat::DEF
        .scenes
        .iter()
        .find(|scene| scene.name == "chat/mockup")
        .ok_or("no chat/mockup scene")?
        .setup;
    let hub = Hub::new(&[], Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    let hub = hub.try_borrow()?;
    let thread = yd_ui::shown::of_pane(&hub, SCENE_PANE).ok_or("no thread")?;
    let lines = super::wire::lines(&hub.model, Some(thread));
    let agent: Vec<&Row> = lines
        .iter()
        .filter(|row| row.words("source") == Some("agent"))
        .collect();
    assert!(!agent.is_empty(), "no line for what the agent ran");
    let said: Vec<&str> = agent
        .iter()
        .filter_map(|row| row.words("message"))
        .collect();
    assert!(
        said.iter()
            .any(|m| m.starts_with("read") && m.contains("src/routes/upload.rs")),
        "{said:?}"
    );
    let times: Vec<&str> = agent.iter().filter_map(|row| row.words("ts")).collect();
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]), "{times:?}");
    assert!(lines.iter().all(|row| row.id.as_str().starts_with("wire-")));
    let once = super::wire::lines(&hub.model, Some(thread));
    assert_eq!(
        lines, once,
        "an id must be the same from one view to the next"
    );
    Ok(())
}

#[test]
fn the_pane_asks_its_source_for_lines_once_when_it_is_first_laid_out() -> Check {
    use std::cell::RefCell;
    use std::rc::Rc;

    use yd_core::feed::SourceCmd;
    use yd_core::msg::Effect;

    static DEFS: [&PaneDef; 1] = [&DEF];
    let sent = Rc::new(RefCell::new(Vec::new()));
    let hub = Hub::new(&DEFS, Vec::new());
    {
        let into = sent.clone();
        hub.try_borrow_mut()?
            .set_sink(move |effect| into.borrow_mut().push(effect));
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (700.0, 420.0),
        Env::new(true, 1.0),
    );
    for width in [700.0, 640.0, 700.0] {
        rt.set_viewport((width, 420.0), 1.0);
        for _ in 0..3 {
            rt.frame(0.016);
        }
    }
    let sent = sent.borrow();
    assert_eq!(*sent, [Effect::Source(FEED, SourceCmd::Refresh)]);
    Ok(())
}

/// Past its caps the model's lines say so where they cut: older calls, and a call's output.
#[test]
fn the_models_caps_name_what_they_left_out() -> Check {
    use std::collections::BTreeMap;

    use yd_core::ids::{Millis, ThreadId};
    use yd_core::model::Model;
    use yd_core::reduce::threads::Thread;
    use yd_core::reduce::turns::{CallId, ToolCall};

    use super::wire::{CALLS, PRINTED, lines};

    let mut thread = Thread::new(ThreadId::new("t"));
    for n in 0..CALLS as u64 + 5 {
        let mut call = ToolCall {
            id: CallId::new(format!("c{:03}", 999 - n)),
            name: "bash".to_owned(),
            started: Some(Millis(1_000 + n)),
            ..ToolCall::default()
        };
        if n == CALLS as u64 + 4 {
            let said: Vec<String> = (0..PRINTED + 6).map(|k| format!("out {k}")).collect();
            let block = BTreeMap::from([("type", "text".to_owned()), ("text", said.join("\n"))]);
            let mut store = rune::prefs::Prefs::memory();
            if store.set("v", &vec![block]).is_ok() {
                call.output = store.get("v").unwrap_or_default();
            }
        }
        thread.transcript.tools.insert(call.id.clone(), call);
    }
    let rows = lines(&Model::default(), Some(&thread));
    let said: Vec<&str> = rows.iter().filter_map(|row| row.words("message")).collect();
    assert!(
        said.contains(&"[…] 5 older tool calls are not listed (CALLS 40)"),
        "{said:?}"
    );
    assert!(
        said.contains(&"[…] 6 more output lines of bash are not listed (PRINTED 4)"),
        "{said:?}"
    );
    let listed = said.iter().filter(|m| m.starts_with("out ")).count();
    assert_eq!(listed, PRINTED);
    Ok(())
}

/// A list kept and extended line by line must be the list made afresh: the incremental path
/// is an optimisation, and a wrong window would show wrong lines.
#[test]
fn lines_appended_one_by_one_make_the_list_a_fresh_one_would() -> Check {
    fn stuffed(hub: &mut Hub) {
        let rows: Vec<Row> = (0..WINDOW as u64 + 3)
            .map(|n| {
                line(
                    n,
                    if n % 7 == 0 { "WARN" } else { "INFO" },
                    "yi",
                    &format!("line {n}"),
                )
            })
            .collect();
        step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Reset(rows)));
        let state = hub.panes.get_mut::<State>(Some(SCENE_PANE));
        if let Some(state) = state {
            state.asked = true;
        }
    }
    let mut rt = draw(stuffed)?;
    for n in 0..5_u64 {
        let at = WINDOW as u64 + 3 + n;
        let level = if n % 2 == 0 { "ERROR" } else { "WARN" };
        let row = line(at, level, "api", &format!("appended {at}"));
        rt.update(|win, _tx| {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
            }
        });
        rt.frame(0.016);
    }
    let hub = rt.state.hub.try_borrow()?;
    let state = hub.panes.get::<State>(Some(SCENE_PANE)).ok_or("no state")?;
    let made = state.made.try_borrow()?;
    let kept = made.as_ref().ok_or("nothing was kept")?;
    let fresh = shown(hub.model.tables.get(FEED), &[], &Filters::default(), "");
    assert_eq!(ids(&kept.shown), ids(&fresh));
    assert_eq!(
        (
            kept.shown.older,
            kept.shown.matched,
            kept.shown.errors,
            kept.shown.warnings
        ),
        (fresh.older, fresh.matched, fresh.errors, fresh.warnings)
    );
    let notice = |s: &super::shown::Shown| {
        s.table
            .get(0)
            .and_then(|r| r.words("message").map(str::to_owned))
    };
    assert_eq!(notice(&kept.shown), notice(&fresh));
    Ok(())
}

/// Not part of the gate: `cargo test --release -p yd-panes appending -- --ignored --nocapture`
/// prints what one appended line costs a frame at the window size and past it.
#[test]
#[ignore = "a measurement for an optimised build, not a check"]
fn appending_a_line_at_the_window_size_costs_a_frame() -> Check {
    fn stuffed(hub: &mut Hub) {
        let rows: Vec<Row> = (0..WINDOW as u64)
            .map(|n| line(n, "INFO", "yi", &format!("line {n} of the window")))
            .collect();
        step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Reset(rows)));
    }
    let mut rt = draw(stuffed)?;
    for (round, extra) in [(0_u64, 200_u64), (1, 200)] {
        let started = std::time::Instant::now();
        for n in 0..extra {
            let at = WINDOW as u64 + round * extra + n;
            let row = line(at, "INFO", "yi", &format!("appended {at}"));
            rt.update(|win, _tx| {
                if let Ok(mut hub) = win.hub.try_borrow_mut() {
                    step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
                }
            });
            rt.frame(0.016);
        }
        let each = started.elapsed().as_secs_f64() * 1000.0 / extra as f64;
        println!("append at the window size: {each:.2} ms a frame over {extra} lines");
    }
    Ok(())
}
