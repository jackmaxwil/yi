//! A child agent's conversation in a chat pane of its own, driven through the window from two
//! real servers' recordings: a child that is stopped, and one that asks its parent.

use std::path::PathBuf;

use yd_core::grid::saved::BLANK;
use yd_core::ids::ThreadId;
use yd_core::pane::PaneKind;
use yd_core::wire::replay::play;
use yd_panes::chat::pane::child::{Children, OPEN};
use yd_ui::grid_view::view::{Act, act, show_pane};

use super::*;
use crate::windows::hub::{Launch, specs};
use crate::windows::places::Places;

const STOPPED: &str = include_str!("../../../../../../fixtures/wire/child-turn.jsonl");
const ASKS: &str = include_str!("../../../../../../fixtures/wire/child-ask.jsonl");
const SLEEPER: &str = "sub-cbcfaa6f";
const ASKER: &str = "sub-655c3f0b";
/// What only the child's own conversation holds: the brief Yi hands it.
const BRIEF: &str = "[task from parent]";

fn lines(recording: &str, from: usize, to: usize) -> String {
    let lines = recording.lines().skip(from).take(to.saturating_sub(from));
    lines.collect::<Vec<_>>().join("\n")
}

/// The thread a recording made: what its `session/new` answered.
fn made(recording: &str) -> Result<ThreadId, Box<dyn Error>> {
    let answer = recording.lines().nth(3).ok_or("short recording")?;
    let at = answer.find("\"sessionId\":\"").ok_or("no session")? + 13;
    let id = answer.get(at..at + 36).ok_or("short id")?;
    Ok(ThreadId::new(id))
}

/// Plays more of the recording into the running window, as its reader thread would.
fn feed(runtime: &mut Runtime<Win>, recording: &str, from: usize, to: usize) {
    let part = lines(recording, from, to);
    runtime.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let _ = play(&mut hub.model, &part);
        }
    });
    rest(runtime);
}

/// The app's window, wide enough for a chat split in two, over the first `to` recorded lines
/// with their thread shown.
fn window(recording: &str, to: usize) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    let thread = made(recording)?;
    {
        let mut hub = win.hub.try_borrow_mut()?;
        play(&mut hub.model, &lines(recording, 0, to))?;
        yd_ui::shown::pick(&mut hub, root::MAIN, thread);
    }
    let mut runtime = Runtime::new(win, root::view, (1600.0, 900.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    beside_agents(&mut runtime);
    Ok(runtime)
}

/// Chat alone, as the app opens blank, then the Agents pane shown beside it.
fn beside_agents(runtime: &mut Runtime<Win>) {
    rest(runtime);
    runtime.update(|win, tx| {
        act(win, tx, Act::Show(BLANK.slug));
        show_pane(win, tx, PaneKind::Agents);
    });
    rest(runtime);
}

fn keys(runtime: &Runtime<Win>) -> Vec<String> {
    let shown = runtime.elements();
    let live = shown.into_iter().filter(|e| !e.exiting);
    live.filter_map(|e| e.key).collect()
}

/// The key of the one pane whose key starts `slug-` and that holds `part`.
fn pane_of(runtime: &Runtime<Win>, slug: &str, part: &str) -> Result<String, Box<dyn Error>> {
    let keys = keys(runtime);
    let found = keys
        .iter()
        .find(|key| key.starts_with(slug) && key.ends_with(part));
    let found = found.ok_or_else(|| format!("no {slug}…{part}"))?;
    Ok(found.strip_suffix(part).unwrap_or(found).to_owned())
}

fn child_pane(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    pane_of(runtime, "chat-", ".child")
}

fn bound(runtime: &Runtime<Win>) -> Result<usize, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    Ok(hub.panes.get::<Children>(None).map_or(0, |c| c.0.len()))
}

fn asked_for(lines: &Rc<RefCell<Vec<String>>>, method: &str, child: &str) -> usize {
    let lines = lines.borrow();
    let mine = lines
        .iter()
        .filter(|l| l.contains(method) && l.contains(child));
    mine.count()
}

#[test]
fn a_child_row_in_the_agents_pane_opens_its_own_chat_beside_the_parents() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let sent = sent(&runtime)?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    let parent = pane_of(&runtime, "chat-", ".composer")?;
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    let child = child_pane(&runtime)?;
    assert_ne!(child, parent, "a second chat pane");
    let header = row_text(&runtime.elements(), &format!("{child}.child"))?;
    assert!(header.contains("sleeper"), "named: {header:?}");
    assert!(
        header.contains("waiting") || header.contains("writing"),
        "{header:?}"
    );
    assert_eq!(
        asked_for(&sent, "_yi/child_replay", SLEEPER),
        1,
        "its turns asked for"
    );
    let (px, _) = centre(&runtime, &format!("{parent}.conversation"))?;
    let (cx, _) = centre(&runtime, &format!("{child}.conversation"))?;
    assert!((px - cx).abs() > 100.0, "beside the parent's, not over it");
    feed(&mut runtime, STOPPED, 129, 141);
    let turns = row_text(&runtime.elements(), &format!("{child}.conversation"))?;
    assert!(
        turns.contains("Run the bash command"),
        "the child's own turns: {turns:?}"
    );
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    assert_eq!(
        bound(&runtime)?,
        1,
        "asked again, the open pane comes forward"
    );
    Ok(())
}

#[test]
fn return_on_the_selected_agent_and_its_spawn_step_open_the_same_chat() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let parent = pane_of(&runtime, "chat-", ".composer")?;
    let step = format!("{parent}.spawned.{SLEEPER}");
    tap(&mut runtime, &step)?;
    assert_eq!(bound(&runtime)?, 1, "the spawn step opens it");
    let child = child_pane(&runtime)?;
    tap(&mut runtime, &format!("{child}.parent"))?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    tap(&mut runtime, &format!("{agents}.rows"))?;
    strike(&mut runtime, Key::Down, Mods::default());
    strike(&mut runtime, Key::Down, Mods::default());
    strike(&mut runtime, Key::Enter, Mods::default());
    assert_eq!(bound(&runtime)?, 1, "Return on its row finds the same pane");
    let open = Run::new(OPEN).on(Target::App);
    runtime.update(move |win, tx| {
        yd_ui::menu::command_box::run(win, tx, open.clone());
    });
    rest(&mut runtime);
    assert_eq!(bound(&runtime)?, 1, "and so does Open Agent Chat");
    Ok(())
}

#[test]
fn a_child_tagged_update_never_lands_in_the_parents_turns() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let parent = pane_of(&runtime, "chat-", ".composer")?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    feed(&mut runtime, STOPPED, 129, STOPPED.lines().count());
    let child = child_pane(&runtime)?;
    let shown = runtime.elements();
    let theirs = row_text(&shown, &format!("{parent}.conversation"))?;
    let its = row_text(&shown, &format!("{child}.conversation"))?;
    assert!(!theirs.contains(BRIEF), "the parent's turns: {theirs:?}");
    assert!(its.contains("I only have"), "the child's: {its:?}");
    let hub = runtime.state.hub.try_borrow()?;
    let thread = hub
        .model
        .wire
        .threads
        .get(&made(STOPPED)?)
        .ok_or("no thread")?;
    let said = format!("{:?}", thread.transcript.messages);
    assert!(!said.contains(BRIEF), "nor the parent's transcript");
    Ok(())
}

#[test]
fn stop_aborts_the_child_and_the_ended_child_keeps_its_pane_read_only() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let sent = sent(&runtime)?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    let child = child_pane(&runtime)?;
    assert!(
        !live(&runtime, &format!("{child}.composer")),
        "no prompt for a child"
    );
    let note = row_text(&runtime.elements(), &format!("{child}.driven"))?;
    assert!(note.contains("parent thread drives"), "{note:?}");
    tap(&mut runtime, &format!("{child}.stop"))?;
    assert_eq!(asked_for(&sent, "_yi/child_abort", SLEEPER), 1);
    feed(&mut runtime, STOPPED, 129, STOPPED.lines().count());
    let header = row_text(&runtime.elements(), &format!("{child}.child"))?;
    assert!(header.contains("stopped"), "its exit: {header:?}");
    assert!(
        !live(&runtime, &format!("{child}.stop")),
        "nothing left to stop"
    );
    let note = row_text(&runtime.elements(), &format!("{child}.driven"))?;
    assert!(note.contains("stays here to read"), "{note:?}");
    Ok(())
}

#[test]
fn a_childs_question_and_approval_are_answered_in_its_own_pane() -> Check {
    let mut runtime = window(ASKS, 130)?;
    let sent = sent(&runtime)?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    tap(&mut runtime, &format!("{agents}.row.{ASKER}"))?;
    let child = child_pane(&runtime)?;
    // The recorded client's answer to the child's ask is left out: this window gives its own.
    feed(&mut runtime, ASKS, 131, 135);
    let card = format!("{child}.asks.row.");
    let run = keys(&runtime)
        .into_iter()
        .find(|k| k.starts_with(&card) && k.ends_with(".approval.allow"));
    let run = run.ok_or("its approval is not in its pane")?;
    tap(&mut runtime, &run)?;
    assert!(
        sent.borrow().iter().any(|l| l.contains("allow_once")),
        "allowed from its pane"
    );
    feed(&mut runtime, ASKS, 135, 177);
    let question = row_text(&runtime.elements(), &format!("{child}.question"))?;
    assert!(
        question.contains("Which file should I read"),
        "{question:?}"
    );
    tap(&mut runtime, &format!("{child}.composer.input"))?;
    strike(&mut runtime, Key::Text("README.md".into()), Mods::default());
    strike(&mut runtime, Key::Enter, Mods::default());
    let answers = sent.borrow();
    let answer = answers.iter().find(|l| l.contains("_yi/child_answer"));
    let answer = answer.ok_or("no answer was sent")?;
    assert!(answer.contains(r#""questionId":"asker-1""#), "{answer}");
    assert!(answer.contains(r#""text":"README.md""#), "{answer}");
    Ok(())
}

/// The window layer over `file`, with the recording's thread shown.
fn restarted(file: &PathBuf, recording: &str, to: usize) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    let laptop = rune::Display {
        id: None,
        name: None,
        position: (0, 0),
        size: (3200, 1800),
        scale: 2.0,
        primary: true,
    };
    let _specs = specs(
        &win.hub,
        &[laptop],
        Launch {
            headless: false,
            places: Places::at(file),
        },
    );
    {
        let mut hub = win.hub.try_borrow_mut()?;
        play(&mut hub.model, &lines(recording, 0, to))?;
    }
    let runtime = Runtime::new(win, root::view, (1600.0, 900.0), Env::new(true, 1.0));
    Ok(runtime)
}

#[test]
fn a_restart_reopens_a_childs_pane_and_asks_for_its_turns_again() -> Check {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/df17")
        .join(format!("children-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut first = restarted(&file, STOPPED, 129)?;
    first.poll = Some(root::poll);
    first.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Ok(id) = made(STOPPED)
        {
            yd_ui::shown::pick(&mut hub, root::MAIN, id);
        }
    });
    beside_agents(&mut first);
    let agents = pane_of(&first, "agents-", ".pane-header")?;
    tap(&mut first, &format!("{agents}.row.{SLEEPER}"))?;
    let child = child_pane(&first)?;
    drop(first);
    let mut again = restarted(&file, STOPPED, 129)?;
    let sent = sent(&again)?;
    again.poll = Some(root::poll);
    rest(&mut again);
    assert_eq!(
        child_pane(&again)?,
        child,
        "the same pane, bound to the same child"
    );
    let header = row_text(&again.elements(), &format!("{child}.child"))?;
    assert!(header.contains("sleeper"), "{header:?}");
    let replays = asked_for(&sent, "_yi/child_replay", SLEEPER);
    assert_eq!(replays, 1, "its turns asked for again");
    let _ = std::fs::remove_file(&file);
    Ok(())
}

#[test]
fn a_child_the_server_no_longer_holds_says_so_in_its_pane() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let thread = made(STOPPED)?;
    runtime.update(move |win, tx| {
        let target = yd_core::reduce::step::commands::child_address(
            &thread,
            &yd_core::ids::AgentId::new("no-such-child"),
        );
        if let Ok(target) = target {
            yd_ui::menu::command_box::run(win, tx, Run::new(OPEN).on(Target::Thing(target)));
        }
    });
    rest(&mut runtime);
    feed(&mut runtime, STOPPED, 141, 143);
    let child = child_pane(&runtime)?;
    let said = row_text(&runtime.elements(), &format!("{child}.missing"))?;
    assert!(said.contains("no longer exists"), "{said:?}");
    Ok(())
}

/// The Stop in an opened Agents row aborts that child, as the child pane's own Stop does.
#[test]
fn the_agents_detail_stop_aborts_the_child_while_it_runs() -> Check {
    let mut runtime = window(STOPPED, 129)?;
    let sent = sent(&runtime)?;
    let agents = pane_of(&runtime, "agents-", ".pane-header")?;
    tap(&mut runtime, &format!("{agents}.row.{SLEEPER}"))?;
    let detail = format!("{agents}.detail.{SLEEPER}");
    let stop = keys(&runtime)
        .into_iter()
        .find(|key| key.starts_with(&detail) && key.ends_with("stop"));
    let stop = stop.ok_or_else(|| format!("no Stop in the detail: {:?}", keys(&runtime)))?;
    tap(&mut runtime, &stop)?;
    assert_eq!(asked_for(&sent, "_yi/child_abort", SLEEPER), 1);
    feed(&mut runtime, STOPPED, 129, STOPPED.lines().count());
    let left = keys(&runtime).into_iter();
    let left = left.filter(|key| key.starts_with(&detail) && key.ends_with("stop"));
    assert_eq!(left.count(), 0, "an ended child has no Stop");
    Ok(())
}
