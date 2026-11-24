//! What a person using a list would notice if it broke, on Rune's in-process runtime. This
//! file holds the windows the tests drive and the tests of opening, keys and commands.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd, Table};
use yd_core::ids::{CommandId, FeedId, Millis, PaneId, RowId, WindowId};
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Value};
use yd_core::tokens::Tokens;

use super::{list, part, stream};
use crate::list::row;
use crate::list::state::{ListState, Lists};
use crate::pane::{PaneCx, PaneDef, Stage, placeholder};
use crate::theme::look::colour;
use crate::win::{Hub, Win};

#[cfg(test)]
mod buttons;
#[cfg(test)]
mod drawn;
#[cfg(test)]
mod fast;
#[cfg(test)]
mod scroll;

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;

const PANE: PaneId = PaneId(7);
const SIZE: (f32, f32) = (520.0, 360.0);
/// One frame of a 120 Hz display, in seconds.
const DT: f64 = 1.0 / 120.0;

const DAEMONS: FeedId = FeedId::new("daemons");
const CONTAINERS: FeedId = FeedId::new("containers");
const AGENTS: FeedId = FeedId::new("agents");
const LOGS: FeedId = FeedId::new("logs");
const SERVERS: FeedId = FeedId::new("lsp");
const PROBLEMS: FeedId = FeedId::new("lsp").instance(1);

const RESTART: CommandId = CommandId("daemons.restart");
const UNDECLARED: CommandId = CommandId("daemons.unknown");
/// A second button every table declares.
const SECOND: CommandId = yd_core::address::attach::ATTACH;

fn unbuilt(pane: &PaneCx<'_>, _: &mut Cx<Win>) -> El {
    placeholder(pane, PaneKind::Daemons, "LE")
}

/// A pane table of one: the pane whose commands a detail's buttons are looked up in.
static DEFS: &[&PaneDef] = &[&PaneDef {
    kind: PaneKind::Daemons,
    view: unbuilt,
    scenes: &[],
    commands: &[Command {
        id: RESTART,
        title: "Restart",
        key: Some("cmd+r"),
        menu: None,
        kinds: &[Kind::Service],
        route: Route::Source(DAEMONS),
    }],
    settings: &[],
    tabs: None,
    on_tab: None,
    finder: None,
}];

const fn column(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

const SERVICES: ListDesc = ListDesc {
    kind: Kind::Service,
    columns: &[
        column("state", "", Width::Px(10.0), Text::Plain),
        column("name", "Name", Width::Fr(1.0), Text::Name),
        column("pid", "PID", Width::Px(44.0), Text::Number),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail: about,
    footer: Some("Services on this Mac"),
};

const TREE: ListDesc = ListDesc {
    kind: Kind::Agent,
    columns: &[
        column("state", "", Width::Px(7.0), Text::Plain),
        column("name", "Agent", Width::Px(146.0), Text::Name),
        column("doing", "Doing now", Width::Fr(1.0), Text::Plain),
    ],
    tree: true,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail: about,
    footer: None,
};

const LINES: ListDesc = ListDesc {
    kind: Kind::LogLine,
    columns: &[
        column("time", "", Width::Px(76.0), Text::Faint),
        column("message", "", Width::Fr(1.0), Text::Plain),
    ],
    tree: false,
    height: RowHeight::Line,
    facts_per_line: 3,
    detail: raw,
    footer: None,
};

const CELLS: ListDesc = ListDesc {
    kind: Kind::Container,
    columns: &[
        column("state", "", Width::Px(10.0), Text::Plain),
        column("level", "", Width::Px(32.0), Text::Plain),
        column("name", "Name", Width::Fr(1.0), Text::Name),
        column("stage", "State", Width::Px(58.0), Text::Plain),
        column("cpu", "CPU", Width::Px(68.0), Text::Plain),
        column("touched", "Touched", Width::Px(150.0), Text::Plain),
        column("took", "Took", Width::Px(60.0), Text::Plain),
        column("login", "Login", Width::Px(34.0), Text::Plain),
        column("load", "Load", Width::Px(40.0), Text::Plain),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail: about,
    footer: None,
};

fn about(row: &Row) -> Detail {
    Detail {
        facts: vec![
            (
                "What".to_owned(),
                Value::Text(format!("{} is a service", row.id)),
            ),
            ("PID".to_owned(), Value::Text("4412".to_owned())),
            ("Up".to_owned(), Value::Text("3d 2h".to_owned())),
            (
                "Named".to_owned(),
                Value::Text(row.words("name").unwrap_or_default().to_owned()),
            ),
        ],
        text: None,
        commands: vec![RESTART, SECOND, UNDECLARED],
        note: Some("down for 56 minutes".to_owned()),
        ..Detail::default()
    }
}

/// A sentence too long for one line, more tags than a value draws, buttons and a note.
fn tagged(_: &Row) -> Detail {
    Detail {
        facts: vec![
            (
                "Asked to".to_owned(),
                Value::Text("Write tests for the new limiter and run them all twice.".to_owned()),
            ),
            (
                "Touched".to_owned(),
                Value::Tags((0..30).map(|i| format!("file-{i}.rs")).collect()),
            ),
        ],
        text: None,
        commands: vec![RESTART, SECOND, UNDECLARED],
        note: Some("down for 56 minutes".to_owned()),
        ..Detail::default()
    }
}

fn raw(row: &Row) -> Detail {
    Detail {
        facts: vec![("Time".to_owned(), Value::Text("17:58:03.412".to_owned()))],
        text: Some(format!("{{\"id\":\"{}\",\"level\":\"error\"}}", row.id)),
        commands: Vec::new(),
        note: None,
        ..Detail::default()
    }
}

fn service(id: &str, state: Dot) -> Row {
    Row::new(id)
        .text("name", id)
        .cell("state", Value::Dot(state))
        .text("pid", "4412")
}

fn five() -> Vec<Row> {
    ["a", "b", "c", "d", "e"]
        .map(|id| service(id, Dot::Run))
        .into()
}

fn lines(range: std::ops::Range<usize>) -> Vec<Row> {
    range
        .map(|i| {
            Row::new(format!("r{i}"))
                .text("time", "17:58:03.412")
                .text("message", format!("line {i}"))
        })
        .collect()
}

fn pane(hub: &Hub) -> PaneCx<'_> {
    PaneCx {
        hub,
        pane: PANE,
        focused: true,
        edits: None,
    }
}

fn services(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let table = hub.model.tables.get(DAEMONS);
    list(&pane(hub), PaneKind::Daemons, &SERVICES, table)
}

/// The services with a detail of one fact that holds thirty tags.
fn touched(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    const TOUCHED: ListDesc = ListDesc {
        facts_per_line: 1,
        detail: tagged,
        ..SERVICES
    };
    let table = hub.model.tables.get(DAEMONS);
    list(&pane(hub), PaneKind::Daemons, &TOUCHED, table)
}

/// A pane that swaps its description when the clock moves: the same columns, another detail.
fn swapping(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    const OTHER: ListDesc = ListDesc {
        detail: tagged,
        ..SERVICES
    };
    let desc = if hub.model.now.0 == 0 {
        &SERVICES
    } else {
        &OTHER
    };
    let table = hub.model.tables.get(DAEMONS);
    list(&pane(hub), PaneKind::Daemons, desc, table)
}

fn containers(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let table = hub.model.tables.get(CONTAINERS);
    list(&pane(hub), PaneKind::Containers, &CELLS, table)
}

fn agents(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let table = hub.model.tables.get(AGENTS);
    list(&pane(hub), PaneKind::Agents, &TREE, table)
}

fn logs(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let table = hub.model.tables.get(LOGS);
    list(&pane(hub), PaneKind::Logs, &LINES, table)
}

/// The same lines as a log pane draws them: under its own header, following the tail.
fn tailing(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let table = hub.model.tables.get(LOGS);
    let rows = stream(&pane(hub), PaneKind::Logs, "", &LINES, table);
    col().w_full().h_full().child(rows)
}

/// Two lists in one pane, with the same row ids in both.
fn two(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let (pane, kind) = (pane(hub), PaneKind::LanguageServers);
    let tables = &hub.model.tables;
    col()
        .w_full()
        .h_full()
        .child(part(&pane, kind, "servers", &SERVICES, tables.get(SERVERS)))
        .child(part(
            &pane,
            kind,
            "problems",
            &SERVICES,
            tables.get(PROBLEMS),
        ))
}

/// A list over rows made in the view, as a pane does for rows it derives from other state:
/// three rows whose ids carry the model's clock.
fn derived(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let now = hub.model.now.0;
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(
        (0..3)
            .map(|i| service(&format!("g{now}-{i}"), Dot::Run))
            .collect(),
    ));
    list(&pane(hub), PaneKind::Daemons, &SERVICES, Some(&table))
}

thread_local! {
    static VIEWS: Cell<u32> = const { Cell::new(0) };
}

/// The window's root, as the app's is, counting its runs.
fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let body = match hub.stage {
        Stage::View(view) => view(win, &hub, cx),
        Stage::App | Stage::Pane(_) => col(),
    };
    col().key("root").w_full().h_full().child(body)
}

type StageFn = fn(&Win, &Hub, &mut Cx<Win>) -> El;

fn window_of(stage: StageFn, size: (f32, f32), feeds: Vec<(FeedId, Vec<Row>)>) -> Window {
    let hub = Hub::new(DEFS, Vec::new());
    if let Ok(mut hub) = hub.try_borrow_mut() {
        hub.stage = Stage::View(stage);
        for (feed, rows) in feeds {
            step(&mut hub.model, Msg::Feed(feed, FeedEvent::Reset(rows)));
        }
    }
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), root, size, Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn window(stage: StageFn, feeds: Vec<(FeedId, Vec<Row>)>) -> Window {
    window_of(stage, SIZE, feeds)
}

/// Frames until the window is at rest.
fn settle(rt: &mut Window) {
    let mut frames = 0;
    while rt.frame(DT) && frames < 600 {
        frames += 1;
    }
}

/// Sends `msg` through `step`, as a feed or a command would, and draws.
fn send(rt: &mut Window, msg: Msg) {
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(&mut hub.model, msg);
        }
    });
    settle(rt);
}

fn keyed<'a>(all: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    Ok(all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .ok_or_else(|| format!("no element keyed {key}"))?)
}

fn rect(rt: &Window, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    Ok(keyed(&rt.elements(), key)?.rect)
}

fn drawn(rt: &Window, key: &str) -> bool {
    keyed(&rt.elements(), key).is_ok()
}

/// The element showing exactly `words`.
fn worded<'a>(all: &'a [ElementInfo], words: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    Ok(all
        .iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(words))
        .ok_or_else(|| format!("nothing says {words:?}"))?)
}

fn says(rt: &Window, words: &str) -> bool {
    worded(&rt.elements(), words).is_ok()
}

/// The smallest element with a fill that holds the element `inner`: the box a piece sits in.
fn boxed<'a>(
    all: &'a [ElementInfo],
    inner: &ElementInfo,
) -> Result<&'a ElementInfo, Box<dyn Error>> {
    let holds = |outer: &&ElementInfo| {
        let (o, i) = (outer.rect, inner.rect);
        outer.fill.a > 0.0
            && o[0] <= i[0] + 0.01
            && o[1] <= i[1] + 0.01
            && o[0] + o[2] >= i[0] + i[2] - 0.01
            && o[1] + o[3] >= i[1] + i[3] - 0.01
    };
    Ok(all
        .iter()
        .filter(holds)
        .min_by(|a, b| (a.rect[2] * a.rect[3]).total_cmp(&(b.rect[2] * b.rect[3])))
        .ok_or("no filled box holds it")?)
}

/// The elements of exactly this size, in points.
fn sized(all: &[ElementInfo], w: f32, h: f32) -> Vec<&ElementInfo> {
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    all.iter()
        .filter(|e| !e.exiting && close(e.rect[2], w) && close(e.rect[3], h))
        .collect()
}

/// A press and release inside the element keyed `key`, `down` points under its top.
fn press_at(rt: &mut Window, key: &str, down: f32) -> Check {
    let at = rect(rt, key)?;
    let x = at[0] + (at[2] / 2.0).min(60.0);
    rt.pointer_down(x, at[1] + down);
    rt.pointer_up(x, at[1] + down);
    settle(rt);
    Ok(())
}

fn press(rt: &mut Window, key: &str) -> Check {
    press_at(rt, key, 8.0)
}

fn tap(rt: &mut Window, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    settle(rt);
}

/// The rows of the list keyed `list` that show in its viewport, top to bottom.
fn showing(rt: &Window, list: &str) -> Vec<(String, [f32; 4])> {
    let prefix = format!("{list}.row.");
    let mut rows: Vec<(String, [f32; 4])> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.visible[3] > 0.5)
        .filter_map(|e| Some((e.key.filter(|key| key.starts_with(&prefix))?, e.rect)))
        .collect();
    rows.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]));
    rows
}

fn list_state(rt: &Window, name: &str) -> Result<ListState, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let lists = hub.panes.get::<Lists>(Some(PANE)).ok_or("no list state")?;
    Ok(lists.list(name).ok_or("no such list")?.clone())
}

fn service_key(id: &str) -> String {
    row::key("daemons-7", &RowId::new(id))
}

/// Record 5.2.1 and 5.2.2: the detail is directly under its row, pushes the rows below
/// down, and a second press closes it.
#[test]
fn a_row_opens_under_itself_and_a_second_press_closes_it() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    let (b, c) = (service_key("b"), service_key("c"));
    let (was_b, was_c) = (rect(&rt, &b)?, rect(&rt, &c)?);
    press(&mut rt, &b)?;
    let detail = rect(&rt, "daemons-7.detail.b")?;
    assert_eq!(detail[1], was_b[1] + 20.0, "the detail is under its row");
    assert_eq!((detail[0], detail[2]), (was_b[0], was_b[2]), "full width");
    assert!(detail[3] > 20.0, "the detail is {} tall", detail[3]);
    assert!(says(&rt, "b is a service"), "the detail's facts are drawn");
    assert_eq!(
        rect(&rt, &b)?[3],
        20.0 + detail[3],
        "the row holds its detail"
    );
    assert_eq!(
        rect(&rt, &c)?[1],
        was_c[1] + detail[3],
        "the row below did not move down"
    );
    press(&mut rt, &b)?;
    assert!(
        !drawn(&rt, "daemons-7.detail.b"),
        "a second press left it open"
    );
    assert_eq!(rect(&rt, &c)?, was_c, "the row below did not come back");
    Ok(())
}

/// Record 5.2.2, rules 1 and 4: one slot a list; nothing but a row opens or closes.
#[test]
fn opening_another_row_closes_the_first() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    press(&mut rt, &service_key("d"))?;
    assert!(drawn(&rt, "daemons-7.detail.d"), "d did not open");
    assert!(!drawn(&rt, "daemons-7.detail.b"), "b stayed open beside d");
    for dead in [
        "daemons-7.table-header",
        "daemons-7.footer",
        "daemons-7.detail.d",
    ] {
        press(&mut rt, dead)?;
        assert!(
            drawn(&rt, "daemons-7.detail.d"),
            "a press on {dead} closed the row"
        );
    }
    Ok(())
}

/// The plan's default 8: arrows move the selection, Return opens it, Esc closes it, Home and
/// End jump.
#[test]
fn arrows_move_the_selection_and_return_opens_it() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    rt.update(|_, tx| tx.focus("daemons-7.rows", false));
    settle(&mut rt);
    let cursor =
        |rt: &Window| -> Result<f32, Box<dyn Error>> { Ok(rect(rt, "daemons-7.cursor")?[1]) };
    let top = |rt: &Window, id: &str| -> Result<f32, Box<dyn Error>> {
        Ok(rect(rt, &service_key(id))?[1])
    };
    tap(&mut rt, Key::Down);
    assert_eq!(
        cursor(&rt)?,
        top(&rt, "a")?,
        "the first Down stands on the first row"
    );
    tap(&mut rt, Key::Down);
    assert_eq!(cursor(&rt)?, top(&rt, "b")?);
    assert!(!drawn(&rt, "daemons-7.detail.b"), "an arrow opened a row");
    tap(&mut rt, Key::Enter);
    assert!(
        drawn(&rt, "daemons-7.detail.b"),
        "Return did not open the row"
    );
    tap(&mut rt, Key::Enter);
    assert!(
        !drawn(&rt, "daemons-7.detail.b"),
        "Return on the open row did not close it"
    );
    tap(&mut rt, Key::Enter);
    assert!(drawn(&rt, "daemons-7.detail.b"), "and again opens it");
    tap(&mut rt, Key::Down);
    assert_eq!(cursor(&rt)?, top(&rt, "c")?);
    assert!(
        drawn(&rt, "daemons-7.detail.b"),
        "an arrow closed the open row"
    );
    tap(&mut rt, Key::Escape);
    assert!(!drawn(&rt, "daemons-7.detail.b"), "Esc left the row open");
    tap(&mut rt, Key::End);
    assert_eq!(cursor(&rt)?, top(&rt, "e")?, "End");
    tap(&mut rt, Key::Down);
    assert_eq!(cursor(&rt)?, top(&rt, "e")?, "Down past the last row moved");
    tap(&mut rt, Key::Home);
    assert_eq!(cursor(&rt)?, top(&rt, "a")?, "Home");
    tap(&mut rt, Key::Up);
    assert_eq!(cursor(&rt)?, top(&rt, "a")?, "Up past the first row moved");
    Ok(())
}

/// A press inside the list gives it the keyboard, as a table does.
#[test]
fn a_pressed_list_takes_the_keys() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    tap(&mut rt, Key::Escape);
    assert!(
        !drawn(&rt, "daemons-7.detail.b"),
        "Esc after a press left it open"
    );
    tap(&mut rt, Key::Down);
    assert_eq!(
        rect(&rt, "daemons-7.cursor")?[1],
        rect(&rt, &service_key("c"))?[1],
        "Down did not move on from the pressed row"
    );
    Ok(())
}

/// A button in an opened row runs its command aimed at the row (conventions: Commands,
/// Addresses). The first button is the primary one.
#[test]
fn a_button_in_an_open_row_runs_its_command_on_the_row() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    let sent = Rc::new(RefCell::new(Vec::new()));
    let log = sent.clone();
    rt.state
        .hub
        .try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    press(&mut rt, &service_key("b"))?;
    let all = rt.elements();
    let restart = keyed(&all, "daemons-7.detail.b.daemons.restart")?;
    assert_eq!(restart.rect[3], 20.0, "a button is 20 tall");
    assert_eq!(
        restart.fill,
        colour(Tokens::default().fg),
        "the first is primary"
    );
    assert!(says(&rt, "Restart"), "a button is titled by its command");
    assert!(
        !drawn(&rt, "daemons-7.detail.b.daemons.unknown") && !says(&rt, "daemons.unknown"),
        "a command no table declares is not drawn"
    );
    assert!(
        says(&rt, "down for 56 minutes"),
        "the note after the buttons"
    );
    press_at(&mut rt, "daemons-7.detail.b.daemons.restart", 10.0)?;
    let run = Run::new(RESTART).on(Target::Thing("yi://service/b".parse()?));
    assert_eq!(
        *sent.borrow(),
        [Effect::Source(DAEMONS, SourceCmd::Run(run))]
    );
    assert!(
        drawn(&rt, "daemons-7.detail.b"),
        "running a command closed the row"
    );
    Ok(())
}

/// Conventions, Keys and Addresses: a row's key is `<pane key>.row.<row id>`, and the row
/// stands for the thing at `yi://<kind>/<row id>`.
#[test]
fn a_row_has_a_stable_key_and_stands_for_a_thing() -> Check {
    let id = RowId::new("com.yi.daemon");
    let key = row::key("daemons-3", &id);
    assert_eq!(key, "daemons-3.row.com.yi.daemon");
    assert_eq!(row::id_of(&key), Some(id));
    assert_eq!(row::id_of("daemons-3.table-header"), None);
    let thing = row::thing(
        &SERVICES,
        &service("com.yi.daemon", Dot::Run).text("name", "yi serve"),
    )?;
    assert_eq!(thing.address.to_string(), "yi://service/com.yi.daemon");
    assert_eq!(
        thing.label, "yi serve",
        "a thing is called by its name cell"
    );
    let excerpt = thing.excerpt.ok_or("no excerpt")?;
    assert!(
        excerpt.contains("yi serve") && excerpt.contains("4412"),
        "{excerpt}"
    );
    Ok(())
}

/// Record 5.2.5: a row keeps its place and stays open while its feed updates it.
#[test]
fn an_open_row_stays_open_while_its_feed_updates() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    let renamed = service("b", Dot::Fail).text("name", "b, restarted");
    send(&mut rt, Msg::Feed(DAEMONS, FeedEvent::Upsert(renamed)));
    assert!(
        drawn(&rt, "daemons-7.detail.b"),
        "an update closed the open row"
    );
    assert!(
        says(&rt, "b, restarted"),
        "the row still shows its old name"
    );
    send(
        &mut rt,
        Msg::Feed(DAEMONS, FeedEvent::Status(FeedStatus::Offline)),
    );
    assert!(says(&rt, "b, restarted"), "an offline feed lost its rows");
    assert!(says(&rt, "Offline: showing what was last known"));
    Ok(())
}

/// A pane may change what an opened row shows; the row that is open must show it at once.
#[test]
fn a_new_description_reaches_the_row_that_is_open() -> Check {
    let mut rt = window(swapping, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    assert!(says(&rt, "b is a service"), "the first description's facts");
    send(&mut rt, Msg::Tick(Millis(1)));
    assert!(
        says(&rt, "Asked to"),
        "the open row kept the old description's detail"
    );
    assert!(!says(&rt, "b is a service"));
    Ok(())
}

/// Record 5.2.2, rule 3: each list has its own slot, also two lists in one pane.
#[test]
fn two_lists_in_one_pane_keep_their_own_open_row() -> Check {
    let mut rt = window(two, vec![(SERVERS, five()), (PROBLEMS, five())]);
    press(&mut rt, "lsp-7.servers.row.b")?;
    press(&mut rt, "lsp-7.problems.row.b")?;
    assert!(
        drawn(&rt, "lsp-7.servers.detail.b"),
        "the second list closed the first's row"
    );
    assert!(drawn(&rt, "lsp-7.problems.detail.b"));
    press(&mut rt, "lsp-7.problems.row.d")?;
    assert!(drawn(&rt, "lsp-7.servers.detail.b"));
    assert!(!drawn(&rt, "lsp-7.problems.detail.b"), "one slot a list");
    assert_eq!(list_state(&rt, "servers")?.open(), Some(&RowId::new("b")));
    Ok(())
}

/// A pane may hand the list rows it made itself; when they change, the list shows the new
/// ones under their own keys.
#[test]
fn a_list_over_rows_made_in_the_view_follows_them() -> Check {
    let mut rt = window(derived, Vec::new());
    assert!(drawn(&rt, "daemons-7.row.g0-1"), "the first rows");
    press(&mut rt, "daemons-7.row.g0-1")?;
    assert!(
        drawn(&rt, "daemons-7.detail.g0-1"),
        "a row made in the view opens"
    );
    send(&mut rt, Msg::Tick(Millis(1)));
    assert!(
        drawn(&rt, "daemons-7.row.g1-1"),
        "the rows that replaced them"
    );
    assert!(says(&rt, "g1-2"));
    assert!(
        !drawn(&rt, "daemons-7.row.g0-1"),
        "a row that is gone kept its key"
    );
    Ok(())
}
