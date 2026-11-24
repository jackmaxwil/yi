//! The composer and the keys a person uses on a waiting card, drawn in process.

use std::error::Error;

use rune::Key;
use yd_ui::pane::SCENE_PANE;
use yd_ui::win::Hub;

use super::super::{DEF, scenes};
use super::{answers, draw, has, inside, key, press, texts};

/// Incident: after a switch of thread the sidebar row kept the keyboard and Return pressed it.
#[test]
fn return_answers_the_card_a_newly_shown_thread_waits_on_with_no_click()
-> Result<(), Box<dyn Error>> {
    // The pane showed another thread before this one.
    let mut rt = draw(
        |hub| {
            scenes::dogfood(hub);
            if let Some(chat) = hub.panes.get_mut::<super::super::Chat>(Some(SCENE_PANE)) {
                chat.held = Some(yd_core::ids::ThreadId::new("an-earlier-thread"));
            }
        },
        (336.0, 750.0),
    )?;
    key(&mut rt, Key::Enter, false);
    assert_eq!(answers(&rt)?, [Some("allow_once".to_owned())]);
    Ok(())
}

/// [`scenes::dogfood`] with its project's files listed, as the files source lists them.
fn with_files(hub: &mut Hub) {
    scenes::dogfood(hub);
    for path in [
        "README.md",
        "src/lib.rs",
        "src/main.rs",
        "tests/main_args.rs",
    ] {
        let row = yd_core::row::Row::new(path).text("kind", "file");
        let event = yd_core::feed::FeedEvent::Upsert(row);
        yd_core::msg::step(
            &mut hub.model,
            yd_core::msg::Msg::Feed(yd_core::feed::FILES, event),
        );
    }
}

#[test]
fn an_at_word_finds_a_project_file_and_a_press_attaches_it() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(with_files, (336.0, 750.0))?;
    press(&mut rt, "chat-0.composer.input")?;
    key(&mut rt, Key::Text("see @main".into()), false);
    for _ in 0..4 {
        rt.frame(0.016);
    }
    let found = inside(&rt.elements(), "chat-0.composer.files")?;
    assert_eq!(
        found, "src/main.rs tests/main_args.rs",
        "the finder lists the matches"
    );
    press(&mut rt, "chat-0.composer.files.0")?;
    let hub = rt.state.hub.try_borrow()?;
    let attached: Vec<String> = hub
        .model
        .attached
        .things
        .iter()
        .map(|t| t.address.to_string())
        .collect();
    assert_eq!(attached, ["yi://file/src/main.rs"]);
    drop(hub);
    let field = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("chat-0.composer.input"));
    assert_eq!(
        field.and_then(|e| e.text),
        Some("see ".to_owned()),
        "the @ word goes"
    );
    Ok(())
}

/// The effort is the picker's first group, and a model a thread answered with comes before
/// the rest of a long list.
#[test]
fn the_model_picker_offers_the_effort_first_and_the_used_models_next() -> Result<(), Box<dyn Error>>
{
    static DEFS: [&yd_ui::pane::PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    let mut hub = hub.try_borrow_mut()?;
    scenes::dogfood(&mut hub);
    let thread = hub
        .model
        .wire
        .listed()
        .first()
        .copied()
        .ok_or("no thread")?
        .clone();
    let groups = crate::chat::composer::groups(&hub, &thread, &["thought_level", "model"]);
    let names: Vec<Vec<&str>> = groups
        .iter()
        .map(|g| g.iter().map(|i| i.1.as_str()).collect())
        .collect();
    assert_eq!(
        names,
        [
            vec!["low", "high"],
            vec!["Anthropic · Claude Opus 5.5"],
            vec!["Bedrock · Claude Sonnet 5.5"]
        ]
    );
    Ok(())
}

/// The dogfood relay sent zero usage on every answer: zero is not what was spent.
#[test]
fn a_thread_whose_provider_sends_no_usage_says_so_and_not_zero() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::dogfood, (336.0, 750.0))?;
    let hub = rt.state.hub.try_borrow()?;
    let thread = hub
        .model
        .wire
        .listed()
        .first()
        .copied()
        .ok_or("no thread")?;
    assert_eq!(super::super::meta(thread), "turn 2 · no usage reported");
    drop(hub);
    let mut rt = draw(scenes::stopped, (336.0, 750.0))?;
    press(&mut rt, "chat-0.turn.1")?;
    assert!(
        has(&rt.elements(), "no usage reported"),
        "{}",
        texts(&rt.elements())
    );
    assert!(!has(&rt.elements(), "0 tokens"));
    let queued = inside(
        &draw(scenes::dogfood, (336.0, 750.0))?.elements(),
        "chat-0.step.b1",
    )?;
    assert!(
        queued.ends_with("queued"),
        "a step not yet started is queued: {queued:?}"
    );
    Ok(())
}

type Lines = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

/// [`scenes::commanding`] with a prompt typed while it runs, then Stop pressed and the turn
/// ended as Yi ends it: the request lines the pane sent from the start.
fn stopped_with_a_prompt_queued() -> Result<(rune::Runtime<yd_ui::win::Win>, Lines), Box<dyn Error>>
{
    let mut rt = draw(scenes::commanding, (336.0, 750.0))?;
    let sent = Lines::default();
    let log = std::rc::Rc::clone(&sent);
    rt.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let yd_core::msg::Effect::Wire(request) = effect {
            log.borrow_mut().push(request.line);
        }
    });
    press(&mut rt, "chat-0.composer.input")?;
    key(&mut rt, Key::Text("and the docs".into()), false);
    key(&mut rt, Key::Enter, false);
    assert!(has(&rt.elements(), "queued · and the docs"));
    press(&mut rt, "chat-0.composer.send")?;
    // The turn ends as Yi ends it; what the reducer sends then is what the app would send.
    let ended = {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        yd_core::wire::replay::play_line(&mut hub.model, &scenes::idle())?
    };
    for effect in ended {
        if let yd_core::msg::Effect::Wire(request) = effect {
            sent.borrow_mut().push(request.line);
        }
    }
    rt.invalidate();
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn prompted(sent: &Lines) -> bool {
    let lines = sent.borrow();
    lines
        .iter()
        .any(|l| l.contains("session/prompt") && l.contains("and the docs"))
}

/// Incident: after Stop the prompt typed during the turn went at once, unasked.
#[test]
fn after_stop_a_queued_prompt_waits_and_its_send_sends_it() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = stopped_with_a_prompt_queued()?;
    assert!(!prompted(&sent), "nothing goes after the stop");
    assert!(has(&rt.elements(), "queued · and the docs"));
    assert!(has(
        &rt.elements(),
        "stopped · these wait until you send them"
    ));
    press(&mut rt, "chat-0.composer.queued.send")?;
    assert!(prompted(&sent), "Send sends it");
    assert!(
        !has(&rt.elements(), "stopped · these wait"),
        "and nothing waits for you"
    );
    Ok(())
}

#[test]
fn after_stop_return_in_the_empty_composer_sends_the_queue() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = stopped_with_a_prompt_queued()?;
    press(&mut rt, "chat-0.composer.input")?;
    key(&mut rt, Key::Enter, false);
    assert!(prompted(&sent), "Return sends what waited");
    Ok(())
}

/// Dogfood: a prompt holding a long path ran past its bubble's right edge.
#[test]
fn a_prompt_with_a_word_longer_than_the_pane_wraps_inside_its_bubble() -> Result<(), Box<dyn Error>>
{
    let rt = draw(scenes::long_word, (336.0, 750.0))?;
    let all = rt.elements();
    let words = all.iter().filter(|e| !e.exiting).find(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("See `src/very"))
    });
    let [_, y, _, h] = words.ok_or("no prompt")?.rect;
    let glyphs = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH);
    let inside = glyphs.filter(|i| i.rect[1] >= y && i.rect[1] < y + h);
    let right = inside.fold(0.0_f32, |r, i| r.max(i.rect[0] + i.rect[2]));
    assert!(right <= 336.0, "the words run past the pane to {right}");
    Ok(())
}
