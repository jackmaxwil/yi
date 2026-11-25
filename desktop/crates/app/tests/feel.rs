//! How a long, real-shaped conversation feels: each test prints `feel <name> <p50> <p95> <max>`
//! in milliseconds of this thread's time a frame. Release profile only, via `just feel`.

mod support;

use std::error::Error;
use std::time::Instant;

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use support::long::{ANSWER, answers, feed, fresh, heavy_tool_lines, line, tool_lines};
use support::scene::{WINDOW, timed};
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::msg::Msg;
use yd_core::wire::replay::play_line;
use yd_ui::win::{Win, dispatch};
use yi_desktop::root;

type Check = Result<(), Box<dyn Error>>;

const FRAME: f64 = 1.0 / 120.0;

fn window(lines: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = fresh()?;
    let fold = Instant::now();
    feed(&win, lines)?;
    println!("\nfeel fold {:.3}", fold.elapsed().as_secs_f64() * 1000.0);
    let mut rt = Runtime::new(win, root::view, WINDOW, Env::new(true, 1.0));
    rt.poll = Some(root::poll);
    rt.on_key = Some(root::on_key);
    println!("\nfeel first_frame {:.3}", timed(&mut rt, FRAME));
    for _ in 0..30 {
        rt.frame(FRAME);
    }
    Ok(rt)
}

/// Prints the median, 95th percentile and worst of `frames`; the 95th percentile.
fn report(name: &str, mut frames: Vec<f64>) -> f64 {
    frames.sort_by(f64::total_cmp);
    let at = |q: usize| {
        let index = frames.len().saturating_mul(q) / 100;
        frames.get(index).copied().unwrap_or(0.0)
    };
    let max = frames.last().copied().unwrap_or(0.0);
    println!(
        "\nfeel {name} {:.3} {:.3} {max:.3} n={}",
        at(50),
        at(95),
        frames.len()
    );
    at(95)
}

/// Streams one long answer into the open thread, a small delta a frame.
fn stream_into(name: &str, rt: &mut Runtime<Win>, mut seq: usize, message: usize) -> f64 {
    let message = message.saturating_add(1);
    let mut start = Vec::new();
    for at in [17, 18] {
        seq = seq.saturating_add(1);
        start.push(line(at, message, seq, ""));
    }
    let reply = ANSWER.repeat(3);
    let letters: Vec<char> = reply.chars().collect();
    let mut frames = Vec::new();
    rt.update(move |win: &mut Win, _| {
        let _ = feed(win, &start.join("\n"));
    });
    for chunk in letters.chunks(14) {
        seq = seq.saturating_add(1);
        let delta = line(19, message, seq, &chunk.iter().collect::<String>());
        rt.update(move |win: &mut Win, _| {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                let _ = play_line(&mut hub.model, &delta);
            }
        });
        frames.push(timed(rt, FRAME));
    }
    report(name, frames)
}

fn scroll(name: &str, rt: &mut Runtime<Win>) -> f64 {
    let (x, y) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);
    let mut frames = Vec::new();
    for step in 0..240 {
        let dy = if step < 160 { 40.0 } else { -40.0 };
        rt.wheel_xy(x, y, 0.0, dy);
        frames.push(timed(rt, FRAME));
    }
    report(name, frames)
}

fn hover(name: &str, rt: &mut Runtime<Win>) {
    let mut frames = Vec::new();
    for step in 0..120 {
        let x = 300.0 + (step as f32) * 4.0;
        rt.pointer_move(x, 300.0 + ((step % 7) as f32) * 30.0);
        frames.push(timed(rt, FRAME));
    }
    report(name, frames);
}

fn typing(name: &str, rt: &mut Runtime<Win>) -> Result<f64, Box<dyn Error>> {
    let all = rt.elements();
    let input = all
        .iter()
        .find_map(|e| e.key.clone().filter(|k| k.ends_with(".composer.input")))
        .ok_or("no composer is drawn")?;
    rt.update(move |_, tx| tx.focus(input.clone(), true));
    rt.frame(FRAME);
    let mut frames = Vec::new();
    for letter in "a budget is a number in one table and a feel is a frame".chars() {
        rt.key(KeyEvent {
            key: Key::Text(letter.to_string()),
            mods: Mods::default(),
        });
        frames.push(timed(rt, FRAME));
    }
    Ok(report(name, frames))
}

fn flip(name: &str, rt: &mut Runtime<Win>) {
    let mut frames = Vec::new();
    for _ in 0..6 {
        rt.update(|win: &mut Win, _| {
            dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)))
        });
        for _ in 0..120 {
            let start = Instant::now();
            let more = rt.frame(FRAME);
            frames.push(start.elapsed().as_secs_f64() * 1000.0);
            if !more {
                break;
            }
        }
    }
    report(name, frames);
}

#[test]
fn streaming_into_a_new_thread() -> Check {
    let (lines, seq, message) = answers(1, 0);
    let mut rt = window(&lines)?;
    stream_into("stream_new_thread", &mut rt, seq, message);
    Ok(())
}

#[test]
fn a_long_thread_of_answers() -> Check {
    let (lines, seq, message) = answers(40, 60);
    let mut rt = window(&lines)?;
    // Incident: lines numbered out of order are dropped by the reducer, and this thread once
    // drew nothing while its numbers read as fast.
    assert_eq!(
        drawn(&rt, ".answer"),
        60,
        "the open turn's answers are drawn"
    );
    scroll("scroll_answers", &mut rt);
    typing("type_answers", &mut rt)?;
    flip("flip_answers", &mut rt);
    hover("hover_answers", &mut rt);
    stream_into("stream_answers", &mut rt, seq, message);
    Ok(())
}

#[test]
fn a_turn_of_many_tool_calls() -> Check {
    let (lines, seq) = tool_lines(20);
    let mut rt = window(&lines)?;
    let rows = drawn(&rt, ".step.");
    assert!(rows > 50, "the turn draws {rows} tool rows");
    scroll("scroll_tools", &mut rt);
    typing("type_tools", &mut rt)?;
    flip("flip_tools", &mut rt);
    hover("hover_tools", &mut rt);
    stream_into("stream_tools", &mut rt, seq, 9_000);
    Ok(())
}

#[test]
fn a_turn_of_many_tool_calls_with_life_sized_outputs() -> Check {
    let (lines, seq) = heavy_tool_lines(25, 30_000);
    let mut rt = window(&lines)?;
    let rows = drawn(&rt, ".step.");
    assert!(rows > 50, "the turn draws {rows} tool rows");
    let scrolled = scroll("scroll_heavy", &mut rt);
    let typed = typing("type_heavy", &mut rt)?;
    flip("flip_heavy", &mut rt);
    hover("hover_heavy", &mut rt);
    let streamed = stream_into("stream_heavy", &mut rt, seq, 9_000);
    // What `tests/budgets/run.py` judges: the 95th percentile frame of each.
    println!("\nbudget scroll_long_turn_ms {scrolled:.3}");
    println!("\nbudget type_long_turn_ms {typed:.3}");
    println!("\nbudget stream_long_turn_ms {streamed:.3}");
    Ok(())
}

/// How many drawn elements have `part` in their key.
fn drawn(rt: &Runtime<Win>, part: &str) -> usize {
    let all = rt.elements();
    let keyed = all.iter().filter_map(|e| e.key.as_deref());
    keyed.filter(|key| key.contains(part)).count()
}

/// The drawn and the laid-out height of the streaming answer.
fn answer_heights(rt: &Runtime<Win>) -> Result<(f32, f32), Box<dyn Error>> {
    let all = rt.elements();
    let answer = all
        .iter()
        .rfind(|e| e.key.as_deref().is_some_and(|k| k.ends_with(".answer")))
        .ok_or("no answer is drawn")?;
    Ok((answer.rect[3], answer.target[3]))
}

#[test]
fn a_streaming_answer_glides_to_its_height_and_lands_on_it() -> Check {
    let (lines, mut seq, message) = answers(1, 0);
    let mut rt = window(&lines)?;
    let message = message.saturating_add(1);
    let mut arriving = Vec::new();
    for (at, words) in [(17, ""), (18, ""), (19, ANSWER)] {
        seq = seq.saturating_add(1);
        arriving.push(line(at, message, seq, words));
    }
    rt.update(move |win: &mut Win, _| {
        let _ = feed(win, &arriving.join("\n"));
    });
    rt.frame(FRAME);
    let (drawn, whole) = answer_heights(&rt)?;
    assert!(whole > 200.0, "the answer lays out {whole} tall");
    assert!(
        drawn < whole - 20.0,
        "a whole answer arriving at once is drawn {drawn} of {whole} on its first frame: it snapped"
    );
    // A row still streaming holds its last line back; whole, it lands on its height.
    let mut ending = Vec::new();
    for at in [21, 22] {
        seq = seq.saturating_add(1);
        ending.push(line(at, message, seq, ANSWER));
    }
    rt.update(move |win: &mut Win, _| {
        let _ = feed(win, &ending.join("\n"));
    });
    for _ in 0..240 {
        rt.frame(FRAME);
    }
    let (drawn, whole) = answer_heights(&rt)?;
    assert!(
        (drawn - whole).abs() < 0.5,
        "two seconds after it ended the answer is drawn {drawn} of {whole}"
    );
    Ok(())
}
