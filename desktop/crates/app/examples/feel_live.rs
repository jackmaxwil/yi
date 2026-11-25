//! The long conversation in a real window, driven on the wall clock: scroll, layout flips,
//! then a streamed answer. Run by `tools/harness/feel.py`, which reads Rune's frame log.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

#[allow(dead_code, reason = "the example uses a part of the tests' builder")]
#[path = "../tests/support/long.rs"]
mod long;

use std::io::Write;
use std::thread::sleep;
use std::time::{Duration, Instant};

use rune::AppHandle;
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::wire::replay::play_line;
use yd_ui::win::{Win, dispatch};

const TICK: Duration = Duration::from_millis(8);

fn conversation(win: &Win) -> Option<String> {
    let hub = win.hub.try_borrow().ok()?;
    let chat = hub
        .model
        .grid
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Chat)?;
    Some(format!(
        "{}-{}.conversation",
        PaneKind::Chat.slug(),
        chat.id.0
    ))
}

fn drive(app: &AppHandle<Win>, seq: usize, marks: &mut Vec<(String, f64)>) {
    let began = Instant::now();
    let mut mark = |name: &str| marks.push((name.to_owned(), began.elapsed().as_secs_f64()));
    mark("settle");
    sleep(Duration::from_millis(1500));
    mark("scroll");
    for step in 0..360u16 {
        let up = f32::from(if step < 180 {
            step
        } else {
            360u16.saturating_sub(step)
        });
        app.update(move |win, tx| {
            if let Some(key) = conversation(win) {
                tx.scroll_to_instant(key, 0.0, 100_000.0 - up * 40.0);
            }
        });
        sleep(TICK);
    }
    mark("flip");
    for _ in 0..4 {
        app.update(|win, _| dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT))));
        sleep(Duration::from_millis(700));
    }
    mark("stream");
    let mut seq = seq;
    let message = 9_001;
    let send = |text: String| {
        app.update(move |win, _| {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                let _ = play_line(&mut hub.model, &text);
            }
        });
    };
    for at in [17, 18] {
        seq = seq.saturating_add(1);
        send(long::line(at, message, seq, ""));
    }
    let reply = long::ANSWER.repeat(3);
    let letters: Vec<char> = reply.chars().collect();
    for chunk in letters.chunks(14) {
        seq = seq.saturating_add(1);
        send(long::line(
            19,
            message,
            seq,
            &chunk.iter().collect::<String>(),
        ));
        sleep(TICK);
    }
    mark("rest");
    sleep(Duration::from_millis(1000));
    mark("end");
}

fn main() {
    let (lines, seq) = long::tool_lines(20);
    // On top and not keyed: a covered window draws no frame, and the run must not take the keys.
    let options = rune::WindowOptions {
        transparent: true,
        always_on_top: true,
        ..rune::WindowOptions::default()
    };
    let app = yi_desktop::root::app()
        .window_options(options)
        .init(move || {
            let win = long::fresh().unwrap_or_else(|_| yi_desktop::root::init());
            let _ = long::feed(&win, &lines);
            win
        });
    let handle = app.handle();
    let mut started = false;
    app.on_ready(move |_| {
        if std::mem::replace(&mut started, true) {
            return;
        }
        let app = handle.clone();
        std::thread::spawn(move || {
            let mut marks = Vec::new();
            drive(&app, seq, &mut marks);
            let said: Vec<String> = marks.iter().map(|(n, t)| format!("{n} {t:.4}")).collect();
            let _ = writeln!(std::io::stdout(), "feel-marks {}", said.join(" "));
            app.quit();
        });
    })
    .run();
}
