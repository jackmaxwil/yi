use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_core::wire::replay::play;
use yd_ui::kit::button::Button;
use yd_ui::pane::Stage;
use yd_ui::shown::thread;
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win};

use super::bell::Bell;
use super::scenes::SCENES;
use super::{choices, sync};

type Sent = Rc<RefCell<Vec<String>>>;

fn root(win: &Win, _: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let backdrop = col().key("root").w_full().h_full();
    backdrop
        .fill(colour(hub.model.tokens.window))
        .children(super::layer(&hub))
}

fn window(name: &str) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let scene = SCENES
        .iter()
        .find(|scene| scene.name == name)
        .ok_or("no such scene")?;
    let shared = Hub::new(&[], Vec::new());
    let sent: Sent = Rc::default();
    {
        let mut hub = shared.try_borrow_mut()?;
        hub.stage = Stage::App;
        let log = Rc::clone(&sent);
        hub.set_sink(move |effect| {
            if let Effect::Wire(request) = effect {
                log.borrow_mut().push(request.line);
            }
        });
        (scene.setup)(&mut hub);
    }
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(0)),
        root,
        (1200.0, 750.0),
        Env::new(true, 1.0),
    );
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    Ok((rt, sent))
}

fn bell(rt: &Runtime<Win>) -> Result<Bell, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    Ok(hub.panes.get::<Bell>(None).cloned().ok_or("no bell")?)
}

fn texts(rt: &Runtime<Win>) -> Vec<String> {
    let all = rt.elements();
    all.into_iter()
        .filter(|element| !element.exiting)
        .filter_map(|element| element.text)
        .collect()
}

fn find(all: &[ElementInfo], key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    Ok(hit.ok_or(format!("no element keyed {key}"))?.rect)
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Result<(), Box<dyn Error>> {
    let [x, y, w, h] = find(&rt.elements(), key)?;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    Ok(())
}

#[test]
fn the_mockups_rows_come_needs_first_then_finished_then_read() -> Result<(), Box<dyn Error>> {
    let (rt, _) = window("notifications/mockup")?;
    let bell = bell(&rt)?;
    let rows: Vec<(&str, bool)> = bell
        .shown()
        .into_iter()
        .map(|item| (item.title.as_str(), item.read))
        .collect();
    let want = [
        ("Rate limit", false),
        ("Cold start", false),
        ("Checkout", false),
        ("Notes 2.4", true),
    ];
    assert_eq!(rows, want);
    assert!(texts(&rt).iter().any(|text| text == "3 new"));
    Ok(())
}

#[test]
fn an_approval_offers_its_options_and_a_finished_thread_one_review() -> Result<(), Box<dyn Error>> {
    let (rt, _) = window("notifications/mockup")?;
    let hub = rt.state.hub.try_borrow()?;
    let bell = hub.panes.get::<Bell>(None).ok_or("no bell")?;
    let shown = bell.shown();
    let labels = |at: usize| -> Vec<(String, Button)> {
        let found = shown.get(at).map(|item| choices(&hub.model.wire, item));
        let found = found.unwrap_or_default();
        found.into_iter().map(|c| (c.label, c.look)).collect()
    };
    let approval = [
        ("Allow once".to_owned(), Button::Primary),
        ("Always".to_owned(), Button::Plain),
        ("Deny".to_owned(), Button::Ghost),
    ];
    assert_eq!(labels(0), approval);
    let question = [
        ("Answer".to_owned(), Button::Plain),
        ("Open thread".to_owned(), Button::Ghost),
    ];
    assert_eq!(labels(1), question);
    assert_eq!(labels(2), [("Review changes".to_owned(), Button::Plain)]);
    assert_eq!(labels(3), [], "a read row has no buttons");
    Ok(())
}

#[test]
fn allow_once_answers_the_ask_with_the_option_the_server_offered() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window("notifications/mockup")?;
    press(
        &mut rt,
        "notifications.row.rate-limit:approval:perm_rate-limit_1.button-0",
    )?;
    let lines = sent.borrow();
    let reply = lines.iter().find(|line| line.contains("allow_once"));
    assert!(reply.is_some(), "no answer was sent: {lines:?}");
    let hub = rt.state.hub.try_borrow()?;
    let waiting = hub.model.wire.waiting().count();
    assert_eq!(waiting, 0, "the ask is marked answered");
    Ok(())
}

#[test]
fn an_answered_ask_leaves_the_panel() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    press(
        &mut rt,
        "notifications.row.rate-limit:approval:perm_rate-limit_1.button-0",
    )?;
    assert!(
        sync(&mut rt.state).is_empty(),
        "a row that leaves is not news"
    );
    rt.frame(1.0 / 60.0);
    let titles: Vec<String> = bell(&rt)?.shown().iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["Cold start", "Checkout", "Notes 2.4"]);
    Ok(())
}

#[test]
fn mark_all_read_dims_every_row_and_empties_the_count() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    press(&mut rt, "notifications.mark-read")?;
    assert_eq!(bell(&rt)?.unread(), 0);
    rt.frame(1.0 / 60.0);
    let all = rt.elements();
    let dim = all
        .iter()
        .find(|e| e.key.as_deref() == Some("notifications.row.checkout:done"))
        .ok_or("no Checkout row")?;
    assert!(dim.opacity < 1.0, "a read row is dimmed: {}", dim.opacity);
    assert!(texts(&rt).iter().any(|text| text == "0 new"));
    assert!(!texts(&rt).iter().any(|text| text == "Review changes"));
    Ok(())
}

#[test]
fn opening_a_thread_shows_it_and_closes_the_panel() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    press(&mut rt, "notifications.row.cold-start:needs.button-1")?;
    rt.frame(1.0 / 60.0);
    let hub = rt.state.hub.try_borrow()?;
    let shown = thread(&hub, WindowId(0)).map(|thread| thread.id.to_string());
    assert_eq!(shown.as_deref(), Some("cold-start"));
    assert!(!hub.panes.get::<Bell>(None).ok_or("no bell")?.open);
    Ok(())
}

#[test]
fn escape_and_a_press_outside_both_close_the_panel() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    rt.pointer_move(100.0, 400.0);
    rt.pointer_down(100.0, 400.0);
    rt.pointer_up(100.0, 400.0);
    rt.frame(1.0 / 60.0);
    assert!(!bell(&rt)?.open, "a press outside closes it");
    let (mut rt, _) = window("notifications/mockup")?;
    let escape = KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    };
    assert!(super::key(&mut rt.state, &escape));
    assert!(!bell(&rt)?.open, "Esc closes it");
    assert!(
        !super::key(&mut rt.state, &escape),
        "closed, it takes no key"
    );
    Ok(())
}

#[test]
fn a_press_inside_the_panel_does_not_close_it() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    press(&mut rt, "notifications.header")?;
    assert!(bell(&rt)?.open);
    Ok(())
}

#[test]
fn a_panel_with_nothing_in_it_says_so() -> Result<(), Box<dyn Error>> {
    let (rt, _) = window("notifications/empty")?;
    assert!(texts(&rt).iter().any(|text| text == "Nothing needs you"));
    assert!(texts(&rt).iter().any(|text| text == "0 new"));
    Ok(())
}

#[test]
fn escape_closes_a_panel_the_bell_opened() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window("notifications/mockup")?;
    for _ in 0..2 {
        rt.update(|win, tx| {
            super::run(win, tx, super::TOGGLE);
        });
        rt.frame(1.0 / 60.0);
    }
    assert!(bell(&rt)?.open, "closed, then opened again");
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(
        !bell(&rt)?.open,
        "the panel took the key it was focused for"
    );
    Ok(())
}

#[test]
fn a_row_is_news_once_and_a_thread_that_finishes_again_is_news_again() -> Result<(), Box<dyn Error>>
{
    let (mut rt, _) = window("notifications/mockup")?;
    assert!(
        sync(&mut rt.state).is_empty(),
        "nothing changed since the last look"
    );
    rt.state
        .hub
        .try_borrow_mut()?
        .panes
        .get_mut::<Bell>(None)
        .ok_or("no bell")?
        .mark_all_read();
    // The same list one minute on: Checkout finished again, unseen.
    let later = include_str!("seen.jsonl").replace("1794677684000", "1794679184000");
    play(&mut rt.state.hub.try_borrow_mut()?.model, &later)?;
    let news = sync(&mut rt.state);
    let titles: Vec<&str> = news.iter().map(|item| item.title.as_str()).collect();
    assert_eq!(titles, ["Checkout"]);
    assert_eq!(bell(&rt)?.unread(), 1);
    Ok(())
}
