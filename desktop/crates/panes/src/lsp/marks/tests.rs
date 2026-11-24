//! What a person editing a file would notice if the server's marks broke, with no GPU.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Mods, PhysicalInput, Runtime, col};
use yd_core::command::table::Target;
use yd_core::feed::{FeedEvent, LSP, OPEN_FILES, SourceCmd};
use yd_core::ids::WindowId;
use yd_core::msg::{Effect, Msg, step};
use yd_core::row::{Dot, Row, Rows, Tone, Value};
use yd_ui::code::doc::{Cols, Doc, Lang, LineNo};
use yd_ui::code::marks::{Glyph, Marks};
use yd_ui::code::pointer::{CodeEvent, Token};
use yd_ui::win::{Hub, Win};

use super::{APPLY_FIX, DEFINITION, File, HOVER, bar, follow, heard, of};

type Check = Result<(), Box<dyn Error>>;

const PATH: &str = "src/limit.rs";
/// The second character of `café` is two bytes: every column after it differs by one.
const TEXT: &str = "let a = 1;\nlet café = tokens - 1;\n";

fn doc() -> Doc {
    Doc::new(TEXT, Lang::RUST, 1)
}

fn line(n: usize) -> LineNo {
    LineNo::from_index(n.saturating_sub(1))
}

fn problem(n: usize, words: &str, tone: Tone, cols: (usize, usize)) -> Row {
    Row::new(format!("problem:{PATH}:2:{n}"))
        .cell("severity", Value::Badge(words.to_owned(), tone))
        .text("message", "subtracting two values can panic")
        .text("path", PATH)
        .text("line", "2")
        .text("col", cols.0.to_string())
        .text("end_col", cols.1.to_string())
}

fn feed(hub: &mut Hub, rows: Vec<Row>) {
    for row in rows {
        step(&mut hub.model, Msg::Feed(LSP, FeedEvent::Upsert(row)));
    }
}

#[test]
fn a_problem_underlines_its_own_bytes_after_a_wide_character() {
    let hub = Hub::new(&[], Vec::new());
    let mut hub = hub.borrow_mut();
    let hub = &mut *hub;
    feed(hub, vec![problem(0, "Warn", Tone::Warn, (12, 22))]);
    let want = Marks::new().squiggle(line(2), Cols::new(12, 22), Tone::Need);
    assert_eq!(of(&hub.model, PATH, &doc()), want);
}

#[test]
fn an_error_is_drawn_in_the_error_colour_and_a_hint_is_not_an_error() {
    let hub = Hub::new(&[], Vec::new());
    let mut hub = hub.borrow_mut();
    let hub = &mut *hub;
    let rows = vec![
        problem(0, "Error", Tone::Bad, (12, 18)),
        problem(1, "Hint", Tone::Dim, (21, 22)),
    ];
    feed(hub, rows);
    let want = Marks::new()
        .squiggle(line(2), Cols::new(12, 18), Tone::Bad)
        .squiggle(line(2), Cols::new(21, 22), Tone::Dim);
    assert_eq!(of(&hub.model, PATH, &doc()), want);
}

#[test]
fn a_problem_at_the_end_of_a_line_marks_its_last_character() {
    let hub = Hub::new(&[], Vec::new());
    let mut hub = hub.borrow_mut();
    let hub = &mut *hub;
    feed(hub, vec![problem(0, "Error", Tone::Bad, (23, 23))]);
    let want = Marks::new().squiggle(line(2), Cols::new(22, 23), Tone::Bad);
    assert_eq!(of(&hub.model, PATH, &doc()), want);
}

#[test]
fn another_files_problem_and_a_line_the_text_lacks_mark_nothing() {
    let hub = Hub::new(&[], Vec::new());
    let mut hub = hub.borrow_mut();
    let hub = &mut *hub;
    let mut far = problem(0, "Error", Tone::Bad, (1, 3));
    far.id = yd_core::ids::RowId::new(format!("problem:{PATH}:9:0"));
    far = far.text("line", "9");
    feed(hub, vec![far]);
    let mut other = problem(0, "Error", Tone::Bad, (1, 3));
    other.id = yd_core::ids::RowId::new("problem:src/other.rs:2:0");
    feed(hub, vec![other]);
    assert_eq!(of(&hub.model, PATH, &doc()), Marks::new());
}

#[test]
fn a_hint_goes_in_before_its_character_and_a_fix_lights_its_line() {
    let hub = Hub::new(&[], Vec::new());
    let mut hub = hub.borrow_mut();
    let hub = &mut *hub;
    let rows = vec![
        problem(0, "Warn", Tone::Warn, (12, 22)),
        Row::new(format!("hint:{PATH}:2:12")).text("text", ": u32"),
        Row::new(format!("fix:{PATH}:2:0")).text("title", "Use checked_sub"),
    ];
    feed(hub, rows);
    let want = Marks::new()
        .squiggle(line(2), Cols::new(12, 22), Tone::Need)
        .glyph(line(2), Glyph::Fix)
        .hint(line(2), 12, ": u32");
    assert_eq!(of(&hub.model, PATH, &doc()), want);
}

type Sent = Rc<RefCell<Vec<Effect>>>;

fn view(win: &Win, _: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    col().child(bar(&hub, "code-0", PATH, line(2)))
}

fn window(rows: Vec<Row>) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let hub = Hub::new(&[], Vec::new());
    let sent: Sent = Rc::default();
    let log = sent.clone();
    hub.try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    feed(&mut *hub.try_borrow_mut()?, rows);
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        view,
        (500.0, 100.0),
        Env::new(true, 1.0),
    );
    for _ in 0..3 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn asks(sent: &Sent) -> Vec<(String, String)> {
    let runs = sent.borrow();
    let source = runs.iter().filter_map(|effect| match effect {
        Effect::Source(_, SourceCmd::Run(run)) => Some(run),
        _ => None,
    });
    let at = |target: &Target| match target {
        Target::Thing(address) => format!(
            "{}#{}",
            address.id(),
            address.part_text().unwrap_or_default()
        ),
        Target::App | Target::Pane(_) => String::new(),
    };
    source
        .map(|run| (run.command.0.to_owned(), at(&run.target)))
        .collect()
}

fn followed(rt: &mut Runtime<Win>) -> bool {
    let done = Rc::new(std::cell::Cell::new(false));
    let seen = done.clone();
    rt.update(move |win, tx| seen.set(follow(win, tx)));
    done.get()
}

fn token() -> Token {
    Token {
        line: line(2),
        cols: Cols::new(12, 18),
        follows: false,
    }
}

#[test]
fn resting_on_a_token_asks_the_server_once_until_it_restarts() -> Check {
    let (mut rt, sent) = window(vec![])?;
    let doc = doc();
    let file = File {
        surface: "code-0",
        path: PATH,
        doc: &doc,
    };
    for _ in 0..2 {
        rt.update(|win, tx| {
            heard(win, &file, CodeEvent::HoverStart(token()), tx);
        });
    }
    let hovers = asks(&sent);
    assert_eq!(hovers, [(HOVER.0.to_owned(), format!("{PATH}#L2:C12"))]);
    let starting = Row::new("server:rust-analyzer").cell("state", Value::Dot(Dot::Run));
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            feed(&mut hub, vec![starting]);
        }
    });
    rt.update(|win, tx| {
        heard(win, &file, CodeEvent::HoverStart(token()), tx);
    });
    assert_eq!(asks(&sent).len(), 2);
    Ok(())
}

#[test]
fn the_fix_button_applies_the_fix_row_it_is_for() -> Check {
    let rows = vec![
        problem(0, "Warn", Tone::Warn, (12, 22)),
        Row::new(format!("fix:{PATH}:2:0")).text("title", "Use checked_sub"),
    ];
    let (mut rt, sent) = window(rows)?;
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| e.key.as_deref() == Some("code-0.fix-bar.apply"))
        .ok_or("no Fix button")?;
    let [x, y, w, h] = hit.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    assert_eq!(
        asks(&sent),
        [(APPLY_FIX.0.to_owned(), format!("fix:{PATH}:2:0#"))]
    );
    Ok(())
}

#[test]
fn command_click_goes_to_the_definition_once_its_row_has_come() -> Check {
    let (mut rt, sent) = window(vec![])?;
    let doc = doc();
    let file = File {
        surface: "code-0",
        path: PATH,
        doc: &doc,
    };
    rt.update(|win, tx| {
        heard(win, &file, CodeEvent::CommandPress(token()), tx);
    });
    assert_eq!(
        asks(&sent),
        [(DEFINITION.0.to_owned(), format!("{PATH}#L2:C12"))]
    );
    assert!(!followed(&mut rt));
    let answer = Row::new(format!("def:{PATH}:2:12"))
        .text("path", "src/other.rs")
        .text("line", "7");
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            feed(&mut hub, vec![answer]);
        }
    });
    // The answer comes while ⌘ is still down from the click: it opens in the viewer all the
    // same, where a press with ⌘ held would open the file beside.
    rt.physical_input(PhysicalInput::Modifiers(Mods {
        cmd: true,
        ..Mods::default()
    }));
    assert!(followed(&mut rt));
    let hub = rt.state.hub.try_borrow()?;
    let open = hub.model.tables.get(OPEN_FILES).ok_or("no open files")?;
    let opened = open.get(0).and_then(|tab| tab.words("path"));
    assert_eq!(opened, Some("src/other.rs"));
    Ok(())
}
