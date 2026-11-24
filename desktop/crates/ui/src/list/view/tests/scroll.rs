//! Scrolling: the header stays, the keys reach any row, a log follows its tail, and a long
//! list costs what a short one does.

use std::time::Instant;

use rune::Key;
use yd_core::feed::FeedEvent;
use yd_core::ids::RowId;
use yd_core::msg::{Msg, step};

use super::{
    Check, DAEMONS, DT, LOGS, VIEWS, Window, drawn, keyed, lines, list_state, logs, press, rect,
    send, services, settle, showing, tailing, tap, window, window_of,
};
use crate::list::row;
use crate::list::view::KEYS;

/// One frame of a 120 Hz display, in milliseconds: what a frame may cost before it is late.
const BUDGET_MS: f64 = 1000.0 / 120.0;

/// The plan's default 9: every list scrolls and its header stays. Scrolling runs no view.
#[test]
fn the_header_stays_while_the_rows_scroll() -> Check {
    let mut rt = window(services, vec![(DAEMONS, lines(0..200))]);
    let head = rect(&rt, "daemons-7.table-header")?;
    let before = VIEWS.get();
    for _ in 0..30 {
        rt.wheel(200.0, 200.0, -40.0);
        rt.frame(DT);
    }
    settle(&mut rt);
    assert_eq!(VIEWS.get(), before, "the view ran while the list scrolled");
    assert_eq!(
        rect(&rt, "daemons-7.table-header")?,
        head,
        "the header moved"
    );
    let top = showing(&rt, "daemons-7");
    let first = top.first().ok_or("no row shows")?;
    assert_eq!(
        first.0, "daemons-7.row.r60",
        "1,200 points of wheel over 20 px rows"
    );
    assert!(
        first.1[1] >= head[1] + head[3] - 0.01,
        "a row is drawn over the header"
    );
    Ok(())
}

/// The plan's default 8: Home and End jump, also to rows that were never built.
#[test]
fn end_and_home_reach_rows_that_are_out_of_view() -> Check {
    let mut rt = window(services, vec![(DAEMONS, lines(0..200))]);
    rt.update(|_, tx| tx.focus("daemons-7.rows", false));
    settle(&mut rt);
    tap(&mut rt, Key::End);
    let shown = showing(&rt, "daemons-7");
    let last = shown.last().ok_or("no row shows")?;
    assert_eq!(
        last.0, "daemons-7.row.r199",
        "End did not bring the last row into view"
    );
    assert_eq!(
        rect(&rt, "daemons-7.cursor")?[1],
        last.1[1],
        "the selection is not on it"
    );
    tap(&mut rt, Key::Up);
    assert_eq!(list_state(&rt, "")?.cursor(), Some(&RowId::new("r198")));
    tap(&mut rt, Key::Home);
    let shown = showing(&rt, "daemons-7");
    assert_eq!(
        shown.first().map(|row| row.0.as_str()),
        Some("daemons-7.row.r0")
    );
    Ok(())
}

/// The plan's default 9: a log follows its tail unless the reader scrolled up or a row is open.
#[test]
fn a_following_list_keeps_its_newest_row_in_view() -> Check {
    let mut rt = window(tailing, vec![(LOGS, lines(0..200))]);
    let mut next = 200;
    let mut more = |rt: &mut Window| {
        for row in lines(next..next + 10) {
            send(rt, Msg::Feed(LOGS, FeedEvent::Upsert(row)));
        }
        next += 10;
        format!("logs-7.row.r{}", next - 1)
    };
    let last = |rt: &Window| showing(rt, "logs-7").last().map(|row| row.0.clone());
    let newest = more(&mut rt);
    assert_eq!(last(&rt), Some(newest), "the tail is not followed");
    assert!(
        list_state(&rt, "")?.at_tail(),
        "the list does not know it is at its tail"
    );

    rt.wheel(200.0, 200.0, 160.0);
    settle(&mut rt);
    let held = last(&rt);
    let newest = more(&mut rt);
    assert_eq!(
        last(&rt),
        held,
        "new lines moved a list the reader had scrolled up"
    );
    assert!(!list_state(&rt, "")?.at_tail());
    rt.wheel(200.0, 200.0, -100_000.0);
    settle(&mut rt);
    assert_eq!(last(&rt), Some(newest), "back at the end");
    let newest = more(&mut rt);
    assert_eq!(
        last(&rt).as_ref(),
        Some(&newest),
        "at the end the tail is followed again"
    );

    let open = newest;
    press(&mut rt, &open)?;
    let all = rt.elements();
    let (stood, detail) = (
        keyed(&all, &open)?.rect[1],
        keyed(&all, &open.replace(".row.", ".detail."))?,
    );
    assert!(
        detail.visible[3] > detail.rect[3] - 0.5,
        "the newest line's detail opened out of view: {:?}",
        detail.visible
    );
    more(&mut rt);
    assert_eq!(rect(&rt, &open)?[1], stood, "new lines moved an open row");
    press(&mut rt, &open)?;
    let newest = more(&mut rt);
    assert_eq!(
        last(&rt),
        Some(newest),
        "with the row closed the tail is followed again"
    );
    Ok(())
}

/// A table lets its oldest rows go (architecture, Invariants): an open row that went with
/// them must not hold the log still for good.
#[test]
fn a_log_follows_again_once_its_open_row_is_gone() -> Check {
    let mut rt = window(tailing, vec![(LOGS, lines(0..200))]);
    let open = showing(&rt, "logs-7")
        .last()
        .ok_or("no row shows")?
        .0
        .clone();
    press(&mut rt, &open)?;
    let gone = row::id_of(&open).ok_or("the row's key names no row")?;
    send(&mut rt, Msg::Feed(LOGS, FeedEvent::Remove(gone)));
    rt.wheel(200.0, 200.0, -100_000.0);
    settle(&mut rt);
    for row in lines(200..210) {
        send(&mut rt, Msg::Feed(LOGS, FeedEvent::Upsert(row)));
    }
    let last = showing(&rt, "logs-7").last().map(|row| row.0.clone());
    assert_eq!(
        last.as_deref(),
        Some("logs-7.row.r209"),
        "the log stood still"
    );
    Ok(())
}

/// The slowest frame that is slow in every pass: other builds on the machine preempt a
/// frame at random, so each frame counts at its fastest.
fn floor(passes: &[Vec<f64>]) -> f64 {
    let frames = passes.iter().map(Vec::len).min().unwrap_or(0);
    let fastest = |k: usize| {
        passes
            .iter()
            .filter_map(|pass| pass.get(k))
            .copied()
            .fold(f64::INFINITY, f64::min)
    };
    (0..frames).map(fastest).fold(0.0, f64::max)
}

fn median(passes: &[Vec<f64>]) -> f64 {
    let mut all: Vec<f64> = passes.iter().flatten().copied().collect();
    all.sort_by(f64::total_cmp);
    all.get(all.len() / 2).copied().unwrap_or(0.0)
}

fn timed(rt: &mut Window) -> f64 {
    let start = Instant::now();
    rt.frame(DT);
    start.elapsed().as_secs_f64() * 1000.0
}

/// A hundred thousand rows: what a frame builds and reads does not grow with the list, and
/// in an optimised build opening, closing and scrolling each fit one 120 Hz frame.
#[test]
fn a_hundred_thousand_rows_open_close_and_scroll_inside_a_frame() -> Check {
    const ROWS: usize = 100_000;
    let start = Instant::now();
    let mut rt = window_of(logs, (520.0, 360.0), vec![(LOGS, lines(0..ROWS))]);
    let first = start.elapsed().as_secs_f64() * 1000.0;
    let built = rt
        .elements()
        .iter()
        .filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|key| key.starts_with("logs-7.row."))
        })
        .count();
    assert!((20..300).contains(&built), "{built} rows are built");

    let (views, keys) = (VIEWS.get(), KEYS.get());
    let scroll = |rt: &mut Window, by: f32| -> Vec<f64> {
        (0..120)
            .map(|_| {
                rt.wheel(200.0, 200.0, -by);
                timed(rt)
            })
            .collect()
    };
    let fast: Vec<Vec<f64>> = (0..5).map(|_| scroll(&mut rt, 400.0)).collect();
    let slow: Vec<Vec<f64>> = (0..5).map(|_| scroll(&mut rt, 16.0)).collect();
    settle(&mut rt);
    assert_eq!(VIEWS.get(), views, "the view ran while the list scrolled");
    assert_eq!(KEYS.get(), keys, "scrolling read row keys again");
    let top = showing(&rt, "logs-7");
    let reached = top.first().ok_or("no row shows after scrolling")?.0.clone();
    assert_eq!(
        reached, "logs-7.row.r15600",
        "5 x 120 frames of 400 and of 16 points"
    );

    let (mut opens, mut closes) = (Vec::new(), Vec::new());
    for pass in 0..5 {
        let rows = showing(&rt, "logs-7");
        let (key, at) = rows.get(3 + pass).ok_or("too few rows show")?.clone();
        let (mut open, mut close) = (Vec::new(), Vec::new());
        for times in [&mut open, &mut close] {
            rt.pointer_down(at[0] + 60.0, at[1] + 8.0);
            rt.pointer_up(at[0] + 60.0, at[1] + 8.0);
            times.push(timed(&mut rt));
            settle(&mut rt);
        }
        assert!(
            !drawn(&rt, &key.replace(".row.", ".detail.")),
            "{key} did not close"
        );
        opens.push(open);
        closes.push(close);
    }
    let read = KEYS.get() - keys;
    assert!(
        read < 1_000,
        "opening and closing five rows read {read} row keys"
    );

    // Not held to the budget: Rune reads every row's key again when a list's length changes.
    let appends: Vec<Vec<f64>> = (0..5)
        .map(|pass| {
            let row = lines(ROWS + pass..ROWS + pass + 1).pop();
            rt.update(move |win, _| {
                if let (Ok(mut hub), Some(row)) = (win.hub.try_borrow_mut(), row) {
                    step(&mut hub.model, Msg::Feed(LOGS, FeedEvent::Upsert(row)));
                }
            });
            vec![timed(&mut rt)]
        })
        .collect();
    let append = median(&appends);
    println!("list of {ROWS} rows: first frame {first:.2} ms, {built} rows built");
    println!("list of {ROWS} rows: a frame that adds one row: median {append:.2} ms");
    for (name, passes) in [
        ("scroll 400 points a frame", &fast),
        ("scroll 16 points a frame", &slow),
        ("open a row", &opens),
        ("close a row", &closes),
    ] {
        let (worst, mid) = (floor(passes), median(passes));
        println!(
            "list of {ROWS} rows: {name}: median {mid:.3} ms, slowest in every pass {worst:.3} ms"
        );
        // A debug build runs Rune unoptimised, 10 to 20 times slower: only its counts are held.
        if !cfg!(debug_assertions) {
            assert!(
                worst <= BUDGET_MS,
                "{name} took {worst:.3} ms of {BUDGET_MS:.2}"
            );
        }
    }
    Ok(())
}
