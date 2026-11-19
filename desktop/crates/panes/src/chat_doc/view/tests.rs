//! What a reader sees and does in the document, drawn in process with no GPU.

use std::error::Error;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use yd_core::ids::WindowId;
use yd_ui::win::{Hub, Win};

use super::WIDTH;
use super::scene::{self, FORK_ID};

type Check = Result<(), Box<dyn Error>>;

fn draw(setup: fn(&mut Hub)) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&yd_ui::pane::PaneDef; 1] = [&crate::chat::DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    fn root(win: &Win, cx: &mut rune::Cx<Win>) -> rune::El {
        match win.hub.try_borrow() {
            Ok(hub) => super::page(win, &hub, cx),
            Err(_) => rune::col(),
        }
    }
    let mut rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (1192.0, 712.0),
        Env::new(true, 1.0),
    );
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok(rt)
}

/// Whether a live text or label shows `needle`, as the parity check reads them.
fn has(tree: &[ElementInfo], needle: &str) -> bool {
    let shows = |words: Option<&str>| words.is_some_and(|t| t.contains(needle));
    tree.iter()
        .filter(|e| !e.exiting && e.opacity > 0.0)
        .any(|e| shows(e.text.as_deref()) || shows(e.label.as_deref()))
}

fn rect(tree: &[ElementInfo], key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let found = tree.iter().find(|e| e.key.as_deref() == Some(key));
    Ok(found.ok_or_else(|| format!("no element keyed {key}"))?.rect)
}

/// Rests the pointer at the middle of `key` for `secs`.
fn rest(rt: &mut Runtime<Win>, key: &str, secs: f32) -> Check {
    let [x, y, w, h] = rect(&rt.elements(), key)?;
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    let mut left = secs;
    while left > 0.0 {
        rt.frame(0.05);
        left -= 0.05;
    }
    rt.frame(0.016);
    Ok(())
}

#[test]
fn the_document_reads_the_turn_it_is_on_with_its_details_out_of_sight() -> Check {
    let rt = draw(scene::thread)?;
    let tree = rt.elements();
    for words in [
        "Turn 1",
        "Found the upload route",
        "18m ago",
        "Turn 2 · Add the limit",
        "now · 14 seconds of work",
        "Worked for 14 seconds · 4 steps",
        "What changed",
        "Reply, or ask for a change to any section",
    ] {
        assert!(has(&tree, words), "the document should show {words:?}");
    }
    assert!(
        !has(&tree, "3 files in context"),
        "facts wait for the pointer"
    );
    assert!(!has(&tree, "84 lines"), "the steps wait for the pointer");
    let [_, _, w, _] = rect(&tree, "chat-0.chat-column")?;
    assert!((w - WIDTH).abs() < 0.5, "the column is {w}, not {WIDTH}");
    Ok(())
}

#[test]
fn resting_on_the_ask_shows_its_facts_and_actions_and_moves_nothing() -> Check {
    let mut rt = draw(scene::thread)?;
    let before = rect(&rt.elements(), "chat-0.block.steps")?;
    rest(&mut rt, "chat-0.block.ask", 0.6)?;
    let tree = rt.elements();
    for words in [
        "17:57:58",
        "3 files in context",
        "Copy",
        "Attach",
        "Fork from here",
    ] {
        assert!(has(&tree, words), "the ask should show {words:?}");
    }
    assert_eq!(
        rect(&tree, "chat-0.block.steps")?,
        before,
        "a detail moved the page"
    );
    Ok(())
}

#[test]
fn a_pass_quicker_than_the_intent_delay_shows_nothing() -> Check {
    let mut rt = draw(scene::thread)?;
    rest(&mut rt, "chat-0.block.ask", 0.2)?;
    assert!(
        !has(&rt.elements(), "3 files in context"),
        "shown before the delay"
    );
    rt.pointer_move(1.0, 1.0);
    for _ in 0..20 {
        rt.frame(0.05);
    }
    assert!(!has(&rt.elements(), "3 files in context"));
    Ok(())
}

#[test]
fn resting_on_the_steps_line_lists_every_step_with_its_time_and_result() -> Check {
    let mut rt = draw(scene::thread)?;
    rest(&mut rt, "chat-0.block.steps", 0.6)?;
    let tree = rt.elements();
    for words in [
        "17:57:59",
        "84 lines",
        "3 matches in 2 files",
        "+6 −1",
        "    .layer(RateLimit::per_user(LIMIT))",
        "to 17:58:01",
    ] {
        assert!(has(&tree, words), "the pop-over should show {words:?}");
    }
    Ok(())
}

#[test]
fn a_press_on_the_fork_in_the_turn_map_shows_that_thread() -> Check {
    let mut rt = draw(scene::thread)?;
    let key = format!("chat-0.map.fork-{FORK_ID}");
    let [x, y, w, h] = rect(&rt.elements(), &key)?;
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    let hub = rt.state.hub.try_borrow()?;
    let shown = yd_ui::shown::thread(&hub, WindowId(0)).map(|t| t.id.as_str().to_owned());
    assert_eq!(shown.as_deref(), Some(FORK_ID));
    Ok(())
}

#[test]
fn a_press_on_a_turn_in_the_turn_map_brings_that_turn_back_into_view() -> Check {
    let mut rt = draw(scene::thread)?;
    rt.set_viewport((1192.0, 420.0), 1.0);
    rt.frame(0.016);
    let [dx, dy, dw, dh] = rect(&rt.elements(), "chat-0.doc")?;
    rt.wheel(dx + dw / 2.0, dy + dh / 2.0, -4000.0);
    rt.frame(0.016);
    let inside = |rt: &Runtime<Win>| -> Result<bool, Box<dyn Error>> {
        let [_, y, _, h] = rect(&rt.elements(), "chat-0.block.turn.1")?;
        Ok(y >= dy - 0.5 && y + h <= dy + dh + 0.5)
    };
    assert!(!inside(&rt)?, "turn 1 should start off screen");
    let [x, y, w, h] = rect(&rt.elements(), "chat-0.map.turn-1")?;
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    for _ in 0..30 {
        rt.frame(0.05);
    }
    assert!(
        inside(&rt)?,
        "the press should scroll turn 1 into the document"
    );
    Ok(())
}

#[test]
fn words_typed_into_the_document_composer_show_and_send_as_a_prompt() -> Check {
    let mut rt = draw(scene::thread)?;
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = std::rc::Rc::clone(&sent);
    rt.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let yd_core::msg::Effect::Wire(request) = effect {
            log.borrow_mut().push(request.line);
        }
    });
    let [x, y, w, h] = rect(&rt.elements(), "chat-0.composer.input")?;
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    let press = |key| KeyEvent {
        key,
        mods: Mods::default(),
    };
    rt.key(press(Key::Text("ship it".into())));
    rt.frame(0.016);
    assert!(
        has(&rt.elements(), "ship it"),
        "the words typed should show"
    );
    rt.key(press(Key::Enter));
    rt.frame(0.016);
    let lines = sent.borrow();
    let prompt = lines
        .iter()
        .any(|line| line.contains("session/prompt") && line.contains("ship it"));
    // A thread that is busy keeps the prompt until its turn ends, and says so.
    let queued = has(&rt.elements(), "queued · ship it");
    assert!(prompt || queued, "{lines:?}");
    Ok(())
}

/// A thread whose only prompt is a web address no space breaks.
fn unbroken(hub: &mut Hub) {
    let word = format!("https://example.com/{}", "a".repeat(180));
    let thread = scene::THREAD;
    let mut script = scene::Script {
        hub,
        thread,
        seq: 1,
    };
    script
        .at(scene::NOW)
        .line(&scene::recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ))
        .line(&scene::send(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}"#,
        ))
        .line(&scene::recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{thread}","configOptions":[]}}}}"#
        )))
        .set(r#""sessionUpdate":"state_update","state":"running""#);
    script.message(scene::NOW, scene::NOW, &scene::user(&scene::quote(&word)));
}

/// Dies with the prompt's quote cut off at the column's edge: a long address was clipped
/// where the reply's text, which breaks anywhere, shows it whole.
#[test]
fn a_long_word_in_a_prompt_breaks_inside_the_column() -> Check {
    let rt = draw(unbroken)?;
    let tree = rt.elements();
    let [column, _, width, _] = rect(&tree, "chat-0.chat-column")?;
    let long = tree
        .iter()
        .find(|e| {
            e.text
                .as_deref()
                .is_some_and(|t| t.contains("https://example.com/"))
        })
        .ok_or("the prompt is not shown")?;
    let [left, _, wide, high] = long.rect;
    assert!(
        left + wide <= column + width + 0.5 && high > 40.0,
        "one clipped line: {:?} in a column {column} wide {width}",
        long.rect
    );
    Ok(())
}
