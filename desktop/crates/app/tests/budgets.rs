//! Speed budgets: each test prints `budget <name> <number>`; `tests/budgets/run.py` judges them.
//! Release profile only, via `just budgets`: a debug run is 10 to 20 times slower.

mod support;

use std::error::Error;
use std::time::Instant;

use rune::{Key, KeyEvent, Mods, Runtime};
use support::scene::{WINDOW, runtime, timed};
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::feed::{ASSETS, FeedEvent};
use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::row::{Row, rows_from_json};
use yd_core::wire::replay::play;
use yd_ui::win::{Win, dispatch};
use yi_desktop::root;

type Check = Result<(), Box<dyn Error>>;

const ASSET_ROWS: &str = include_str!("../../../fixtures/mockup/assets.rows.json");
const TURN: &str = include_str!("../../../fixtures/wire/turn.jsonl");
const FRAME: f64 = 1.0 / 60.0;

fn report(name: &str, number: f64) {
    println!("\nbudget {name} {number:.3}");
}

/// The window as the app runs it: the poll and the key handler installed.
fn window(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut rt = runtime(name, WINDOW)?;
    rt.poll = Some(root::poll);
    rt.on_key = Some(root::on_key);
    Ok(rt)
}

/// Resident memory of this process in KiB, as `ps` reports it.
#[allow(
    clippy::disallowed_methods,
    reason = "the test reads its own resident size from ps"
)]
fn resident_kib() -> Result<u64, Box<dyn Error>> {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()?;
    Ok(String::from_utf8(out.stdout)?.trim().parse()?)
}

#[test]
fn a_frame_with_eight_panes() -> Check {
    let mut rt = window("shell/everything")?;
    let panes = rt.state.hub.try_borrow()?.model.grid.panes.len();
    assert!(panes >= 8, "the scene holds {panes} panes");
    let mut frames = Vec::new();
    for _ in 0..10 {
        rt.update(|win: &mut Win, _| {
            dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)))
        });
        for _ in 0..90 {
            let start = Instant::now();
            let more = rt.frame(FRAME);
            frames.push(start.elapsed().as_secs_f64() * 1000.0);
            if !more {
                break;
            }
        }
    }
    frames.sort_by(f64::total_cmp);
    println!("{} frames across ten layout changes", frames.len());
    report("frame_eight_panes_ms", frames[frames.len() * 95 / 100]);
    Ok(())
}

fn many_rows(count: usize) -> Result<Vec<Row>, Box<dyn Error>> {
    let template = rows_from_json(ASSET_ROWS)?;
    let rows = (0..count).map(|n| {
        let mut row = template[n % template.len()].clone();
        row.id = yd_core::ids::RowId::new(format!("asset-{n:06}"));
        row
    });
    Ok(rows.collect())
}

#[test]
fn scrolling_one_hundred_thousand_rows() -> Check {
    let at_rest = resident_kib()?;
    let mut rt = window("assets/mockup")?;
    let rows = many_rows(100_000)?;
    rt.update(move |win: &mut Win, _| dispatch(win, Msg::Feed(ASSETS, FeedEvent::Reset(rows))));
    let first = timed(&mut rt, FRAME);
    let mut spread = Vec::new();
    for step in 0..200 {
        let dy = if step < 150 { -120.0 } else { 120.0 };
        rt.wheel(600.0, 400.0, dy);
        spread.push(timed(&mut rt, FRAME));
    }
    // The first few steps build rows nothing has drawn yet; they are timed on their own.
    let warm = spread.drain(..5).fold(0.0, f64::max);
    spread.sort_by(f64::total_cmp);
    let p95 = spread[spread.len() * 95 / 100];
    let drawn = rt.elements().iter().filter(|e| e.key.is_some()).count();
    assert!(drawn < 2_000, "{drawn} elements: the list is not windowed");
    report("scroll_100k_rows_ms", p95);
    report("first_scroll_100k_ms", warm);
    report("open_100k_rows_ms", first);
    report("memory_100k_rows_kib", resident_kib()? as f64);
    report("memory_before_rows_kib", at_rest as f64);
    Ok(())
}

/// `count` turns of the recorded `turn.jsonl`, each with its own sequence numbers and ids.
fn turns(count: usize) -> String {
    let lines: Vec<&str> = TURN.lines().collect();
    let mut out: Vec<String> = lines.iter().take(4).map(|l| (*l).to_owned()).collect();
    for n in 0..count {
        for line in lines
            .iter()
            .take(26)
            .skip(8)
            .filter(|l| !l.contains("\"id\":\"prompt-1\""))
        {
            out.push(renumber(line, n));
        }
    }
    out.join("\n")
}

/// The turn's sequence numbers move up by its event count; ids take the turn's number.
fn renumber(line: &str, turn: usize) -> String {
    let mut text = line.replace("msg_p1", &format!("msg_p{turn}"));
    text = text.replace("\"msg_1\"", &format!("\"msg_{turn}\""));
    if let Some(at) = text.find("\"seq\":") {
        let (head, tail) = text.split_at(at + 6);
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        let rest = tail.get(digits.len()..).unwrap_or("");
        let seq: usize = digits.parse().unwrap_or(0);
        text = format!("{head}{}{rest}", seq + turn * 11);
    }
    text
}

#[test]
fn opening_a_two_thousand_turn_conversation_and_typing_in_it() -> Check {
    let start = Instant::now();
    let win = root::init();
    step(
        &mut win.hub.try_borrow_mut()?.model,
        Msg::Tick(Millis(1_794_679_124_000)),
    );
    play(&mut win.hub.try_borrow_mut()?.model, &turns(2_000)).map_err(|e| e.to_string())?;
    let fold = start.elapsed().as_secs_f64() * 1000.0;
    let kept = win.hub.try_borrow()?.model.wire.listed().len();
    assert_eq!(kept, 1, "one thread came out of the replay");
    let mut rt = Runtime::new(win, root::view, WINDOW, rune::Env::new(true, 1.0));
    rt.poll = Some(root::poll);
    rt.on_key = Some(root::on_key);
    let first = timed(&mut rt, FRAME);
    report("open_2000_turns_ms", fold + first);
    let all = rt.elements();
    let input = all
        .iter()
        .find_map(|e| e.key.clone().filter(|k| k.ends_with(".composer.input")))
        .ok_or("no composer is drawn")?;
    let words: Vec<&str> = all.iter().filter_map(|e| e.text.as_deref()).collect();
    assert!(
        words.iter().any(|t| t.contains("faux")),
        "the conversation shows no reply"
    );
    rt.update(move |_, tx| tx.focus(input.clone(), true));
    rt.frame(FRAME);
    let mut worst: f64 = 0.0;
    for letter in "a budget is a number in one table".chars() {
        rt.key(KeyEvent {
            key: Key::Text(letter.to_string()),
            mods: Mods::default(),
        });
        worst = worst.max(timed(&mut rt, FRAME));
    }
    report("type_in_composer_ms", worst);
    Ok(())
}

fn quiet(name: &str) -> Result<(f64, f64), Box<dyn Error>> {
    let mut rt = window(name)?;
    for _ in 0..120 {
        rt.frame(FRAME);
    }
    let before = rt.stats.commits;
    let mut busy = 0_u32;
    for _ in 0..600 {
        if rt.frame(FRAME) {
            busy += 1;
        }
    }
    Ok((
        rt.stats.commits.saturating_sub(before) as f64,
        f64::from(busy),
    ))
}

#[test]
fn an_idle_window_draws_nothing() -> Check {
    let (commits, busy) = quiet("shell/just-chat")?;
    report("idle_commits", commits);
    report("idle_busy_frames", busy);
    let (_, working) = quiet("shell/everything")?;
    report("busy_frames_with_agents_working", working);
    Ok(())
}
