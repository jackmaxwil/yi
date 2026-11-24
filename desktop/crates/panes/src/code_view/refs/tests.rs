//! What a reader finding references would notice if the peek panel broke.

use std::{cell::RefCell, error::Error, rc::Rc};

use rune::{Env, Mods, Runtime};
use yd_core::{command::table::OPEN_FILE, feed::SourceCmd, ids::WindowId, msg::Effect};
use yd_ui::{code::pointer::Token, pane::PaneDef};

use super::*;

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;
type Sent = Rc<RefCell<Vec<Effect>>>;
static DEFS: &[&PaneDef] = &[&DEF];
const TAKE: &str = "ref:src/middleware/rate_limit.rs:13:12:";

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    alone(win, &hub, cx, PaneKind::Code)
}

fn window(prep: impl FnOnce(&mut Hub)) -> Result<(Window, Sent), Box<dyn Error>> {
    let (hub, sent) = (Hub::new(DEFS, Vec::new()), Sent::default());
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = Stage::Pane(PaneKind::Code);
        prep(&mut hub);
        let seen = sent.clone();
        hub.set_sink(move |effect| seen.borrow_mut().push(effect));
    }
    let size = (600.0, 420.0);
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), root, size, Env::new(true, 1.0));
    settle(&mut rt);
    Ok((rt, sent))
}

fn settle(rt: &mut Window) {
    (0..20).for_each(|_| {
        rt.frame(1.0 / 60.0);
    });
}

fn rect(rt: &Window, key: &str) -> Result<[f32; 4], String> {
    let mut found = rt
        .elements()
        .into_iter()
        .filter(|e| e.key.as_deref() == Some(key));
    found
        .next()
        .map(|e| e.rect)
        .ok_or_else(|| format!("no element keyed {key}"))
}

fn has(rt: &Window, words: &str) -> bool {
    let mut shown = rt.elements().into_iter().filter(|e| !e.exiting);
    shown.any(|e| {
        e.text
            .as_deref()
            .is_some_and(|text| text.starts_with(words))
    })
}

fn pressed(rt: &mut Window, key: &str) -> Result<(), String> {
    let [x, y, w, h] = rect(rt, key)?;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
}

fn typed(rt: &mut Window, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    settle(rt);
}

fn click(rt: &mut Window, line: u32, from: u32, to: u32) {
    let (line, cols) = (LineNo::from_index(line as usize - 1), Cols::new(from, to));
    let token = Token {
        line,
        cols,
        follows: false,
    };
    rt.update(move |win, tx| {
        heard(win, tx, SCENE_PANE, CodeEvent::CommandPress(token));
    });
    settle(rt);
}

fn answer_with(rt: &mut Window, rows: Vec<Row>) {
    let reset = Msg::Feed(LSP, FeedEvent::Reset(rows));
    rt.update(move |win, _| drop(with(win, |hub| Some(step(&mut hub.model, reset)))));
    settle(rt);
}

fn read(path: &str) -> Effect {
    Effect::Source(FILES, SourceCmd::Read(path.into()))
}

/// The file last asked of the files source; what the pane tells the language server is not it.
fn last_read(sent: &Sent) -> Option<Effect> {
    let sent = sent.borrow();
    let mut reads = sent
        .iter()
        .filter(|e| matches!(e, Effect::Source(FILES, _)));
    reads.next_back().cloned()
}

fn row_key(n: u32) -> String {
    format!("code-0.refs.ref.{TAKE}{n}")
}

#[test]
fn a_command_click_asks_for_the_name_by_its_character_column() -> Check {
    let file = Row::new("file:src/lib.rs").text("text", "// héllo\nlet é = take(x);\n");
    let open = Run::new(OPEN_FILE).on(Target::Thing(Address::new(Kind::File, "src/lib.rs")?));
    let (mut rt, sent) = window(|hub| {
        step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(file)));
        step(&mut hub.model, Msg::Command(Route::Model, open));
    })?;
    click(&mut rt, 2, 9, 13);
    let sent = sent.borrow();
    let asked = sent.iter().find_map(|effect| match effect {
        Effect::Source(LSP, SourceCmd::Run(ask)) if ask.command == lsp::REFERENCES => Some(ask),
        _ => None,
    });
    let Some(ask) = asked else {
        return Err(format!("no question sent: {sent:?}").into());
    };
    let Target::Thing(at) = &ask.target else {
        return Err("the question has no place".into());
    };
    assert_eq!(
        (at.id(), at.part_text()),
        ("src/lib.rs", Some("L2:C9")),
        "é is two bytes"
    );
    Ok(())
}

#[test]
fn rows_that_arrive_fill_the_panel_grouped_by_file() -> Check {
    let (rt, _) = window(setup)?;
    assert!(has(&rt, "6 references in 3 files") && has(&rt, "definition"));
    let files = [
        "src/middleware/rate_limit.rs",
        "src/routes/upload.rs",
        "tests/upload.rs",
    ];
    assert!(
        files
            .iter()
            .all(|file| rect(&rt, &format!("code-0.refs.file.{file}")).is_ok())
    );
    assert!(rect(&rt, &row_key(0))?[1] < rect(&rt, &row_key(1))?[1]);
    Ok(())
}

fn found(n: u32, path: &str, line: u32, kind: &str) -> Row {
    Row::new(format!("{TAKE}{n}"))
        .text("path", path)
        .text("line", line.to_string())
        .text("col", "5")
        .text("end_col", "9")
        .text("text", "take(x)")
        .text("kind", kind)
}

/// Dies with a list that runs in the order the server answered: lines 36 and 47, then the
/// definition at line 11, and a file before the one the definition is in.
#[test]
fn references_run_by_file_then_line_with_the_definition_first() -> Check {
    let (mut rt, _) = window(setup)?;
    let rows = vec![
        found(0, "src/lib.rs", 36, "read"),
        found(1, "src/a.rs", 3, "read"),
        found(2, "src/lib.rs", 47, "read"),
        found(3, "src/lib.rs", 11, "definition"),
    ];
    answer_with(&mut rt, rows);
    let at = |n: u32| rect(&rt, &row_key(n)).map(|r| r[1]);
    let order = [3, 0, 2, 1].map(at);
    let ys: Vec<f32> = order.into_iter().collect::<Result<_, _>>()?;
    assert!(
        ys.windows(2).all(|w| w[0] < w[1]),
        "wanted definition 11, then 36, 47, then a.rs; got rows at {ys:?}"
    );
    Ok(())
}

fn told(rt: &mut Window, event: FeedEvent) {
    let msg = Msg::Feed(LSP, event);
    rt.update(move |win, _| drop(with(win, |hub| Some(step(&mut hub.model, msg)))));
    settle(rt);
}

fn server(status: &str) -> FeedEvent {
    FeedEvent::Upsert(Row::new("server:ra").text("status", status))
}

#[test]
fn the_panel_stops_waiting_when_the_source_says_there_is_no_answer() -> Check {
    let late = || FeedEvent::Status(FeedStatus::Error("no answer within 10 s".into()));
    let (mut rt, _) = window(|hub| {
        setup(hub);
        step(&mut hub.model, Msg::Feed(LSP, late()));
    })?;
    click(&mut rt, 7, 11, 17);
    assert!(
        has(&rt, WAITING),
        "what an older question came to is not ours"
    );
    told(&mut rt, server("ready · answered 0 requests, 1 timed out"));
    assert!(has(&rt, WAITING), "a server row is not read for a timeout");
    told(&mut rt, late());
    assert!(has(&rt, "no answer within 10 s") && !has(&rt, WAITING));
    click(&mut rt, 12, 5, 11);
    told(&mut rt, server("ready · answered 1 request, slowest 3 ms"));
    assert!(has(&rt, NOTHING), "an answer that named no place");
    Ok(())
}

#[test]
fn an_answer_the_server_cut_says_so_under_the_list() -> Check {
    let (mut rt, _) = window(setup)?;
    let cut = "[…] last answer kept 500 of 731 references (cap 500); the rest are not shown";
    told(
        &mut rt,
        server(&format!("ready · answered 1 request · {cut}")),
    );
    let (list, note) = (
        rect(&rt, "code-0.refs.list")?,
        rect(&rt, "code-0.refs.cut")?,
    );
    assert!(has(&rt, cut), "the cut is named in the panel");
    assert!(note[1] >= list[1] + list[3], "as its last line");
    Ok(())
}

#[test]
fn attach_all_stops_at_twelve_and_says_so() -> Check {
    let (mut rt, _) = window(setup)?;
    let many = (0..15).map(|n| {
        let row = Row::new(format!("{TAKE}{n}"))
            .text("path", "src/a.rs")
            .text("text", "x");
        row.text("line", (n + 1).to_string())
            .text("col", "1")
            .text("end_col", "2")
    });
    answer_with(&mut rt, many.collect());
    pressed(&mut rt, "code-0.refs.attach-all")?;
    assert_eq!(rt.state.hub.try_borrow()?.model.attached.things.len(), 12);
    assert!(has(&rt, "Attached 12; 3 left out"));
    Ok(())
}

#[test]
fn a_second_click_elsewhere_replaces_the_panel() -> Check {
    let (mut rt, _) = window(setup)?;
    click(&mut rt, 7, 11, 17);
    assert!(has(&rt, "Bucket") && !has(&rt, "take"));
    let head = rect(&rt, "code-0.refs.head")?[1];
    assert!(rect(&rt, "code-0.L7")?[1] < head && head < rect(&rt, "code-0.L8")?[1]);
    Ok(())
}

#[test]
fn pressing_a_reference_opens_its_file() -> Check {
    let (mut rt, sent) = window(setup)?;
    pressed(&mut rt, &row_key(3))?;
    assert_eq!(last_read(&sent), Some(read("tests/upload.rs")));
    Ok(())
}

#[test]
fn the_arrow_keys_move_return_opens_and_escape_closes() -> Check {
    let (mut rt, sent) = window(setup)?;
    pressed(&mut rt, "code-0.refs.head")?;
    [Key::Down, Key::Down, Key::Enter]
        .into_iter()
        .for_each(|key| typed(&mut rt, key));
    assert_eq!(
        last_read(&sent),
        Some(read("tests/upload.rs")),
        "the third reference"
    );
    let (mut rt, _) = window(setup)?;
    pressed(&mut rt, "code-0.refs.head")?;
    typed(&mut rt, Key::Escape);
    assert!(rect(&rt, "code-0.refs.head").is_err(), "the panel is gone");
    Ok(())
}
