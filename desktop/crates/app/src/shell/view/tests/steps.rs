//! The chat pane as a reading column: capped and centred in a wide pane, and its step rows
//! opened in place to show the whole command and what it printed.

use yd_core::pane::PaneKind;

use super::*;

const ALLOW: &str = include_str!("../../../../../../fixtures/wire/approval-allow.jsonl");

/// The key of the chat pane the main window draws.
fn chat(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let docked = grid.window(root::MAIN).ok_or("no window")?.layout.docked();
    let mut panes = docked.map(|(pane, _)| pane);
    let pane = panes.find(|pane| grid.kind(*pane) == Some(PaneKind::Chat));
    Ok(format!("chat-{}", pane.ok_or("no chat pane")?.0))
}

/// The window as a first launch draws it, the chat alone beside the sidebar.
fn alone(runtime: &mut Runtime<Win>) {
    say(runtime, "agents never\nchanges never\nweb never");
    runtime.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            root::blank(&mut hub, root::MAIN);
        }
    });
    rest(runtime);
}

/// Everything an open step shows: its row's words, and the whole of what it printed.
fn opened(runtime: &Runtime<Win>, step: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let words = row_text(&shown, &format!("{step}.detail"))?;
    let output = by_key(&shown, &format!("{step}.output")).and_then(|e| e.text.clone());
    Ok(format!("{words} {}", output.unwrap_or_default()))
}

fn rect(runtime: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let shown = runtime.elements();
    Ok(by_key(&shown, key).ok_or_else(|| format!("no {key}"))?.rect)
}

#[test]
fn a_wide_chat_pane_reads_as_a_capped_column_in_its_middle() -> Check {
    let mut runtime = runtime("shell/everything")?;
    alone(&mut runtime);
    let key = chat(&runtime)?;
    let pane = rect(&runtime, &key)?;
    assert!(pane[2] > 900.0, "the pane is wide: {pane:?}");
    for part in [format!("{key}.composer"), format!("{key}.step.c1")] {
        let [x, _, w, _] = rect(&runtime, &part)?;
        assert!(w <= 770.0, "{part} is {w} wide");
        let middle = x + w / 2.0;
        let centre = pane[0] + pane[2] / 2.0;
        assert!(
            (middle - centre).abs() < 1.0,
            "{part} centred: {middle} vs {centre}"
        );
    }
    Ok(())
}

#[test]
fn a_step_opens_in_place_without_moving_what_is_above_and_return_closes_it() -> Check {
    let mut runtime = runtime("shell/everything")?;
    alone(&mut runtime);
    let key = chat(&runtime)?;
    let (read, grep) = (format!("{key}.step.c1"), format!("{key}.step.c2"));
    assert_eq!(rect(&runtime, &read)?[3], 20.0, "one line closed");
    tap(&mut runtime, &format!("{grep}.verb"))?;
    assert!(live(&runtime, &format!("{grep}.detail")), "it opens");
    let above = rect(&runtime, &read)?[1];
    tap(&mut runtime, &format!("{read}.verb"))?;
    assert!(
        live(&runtime, &format!("{read}.detail")),
        "so does the one above it"
    );
    assert!(
        live(&runtime, &format!("{grep}.detail")),
        "and both stay open"
    );
    assert_eq!(rect(&runtime, &read)?[1], above, "nothing above it moved");
    let words = opened(&runtime, &read)?;
    assert!(words.contains("src/routes/upload.rs"), "{words:?}");
    assert!(words.contains("\n84"), "what it printed: {words:?}");
    runtime.update(move |_, tx| tx.focus(read.clone(), false));
    strike(&mut runtime, Key::Enter, Mods::default());
    assert!(
        !live(&runtime, &format!("{key}.step.c1.detail")),
        "Return closes it"
    );
    assert!(live(&runtime, &format!("{grep}.detail")));
    Ok(())
}

#[test]
fn an_open_command_shows_its_folder_exit_time_and_output_cut_loudly_with_copy() -> Check {
    let printed: String = (1..=4_000).map(|n| format!("line {n}\n")).collect();
    let quoted = format!("{printed:?}");
    let recording = ALLOW.replace(r#""(no output)""#, &quoted);
    let win = root::init();
    {
        let mut hub = win.hub.try_borrow_mut()?;
        yd_core::wire::replay::play(&mut hub.model, &recording)?;
        root::blank(&mut hub, root::MAIN);
    }
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    let clipboard = rune::clipboard::memory();
    runtime.use_clipboard(&clipboard);
    rest(&mut runtime);
    let step = format!("{}.step.c1", chat(&runtime)?);
    assert_eq!(rect(&runtime, &step)?[3], 20.0, "one line closed");
    let above = rect(&runtime, &step)?[1];
    tap(&mut runtime, &format!("{step}.verb"))?;
    let moved = rect(&runtime, &step)?[1];
    assert_eq!(
        moved, above,
        "a conversation that fit does not jump to its new end"
    );
    let words = opened(&runtime, &step)?;
    for part in [
        "echo one > one.txt",
        "$PROJECT",
        "exit 0",
        "73 ms",
        "line 4000",
    ] {
        assert!(words.contains(part), "{part:?} is not shown");
    }
    let total = printed.chars().count();
    let notice = format!("[…] last 20000 of {total} characters shown; Copy takes them all");
    assert!(words.contains(&notice), "{notice:?} is not said");
    assert!(!words.contains("line 1\n"), "the head is cut");
    tap(&mut runtime, &format!("{step}.copy"))?;
    let copied = clipboard.text().map(|text| text.to_string());
    assert_eq!(
        copied.as_deref(),
        Some(printed.as_str()),
        "Copy takes it all"
    );
    Ok(())
}

/// The cap an open row's command and output scroll inside: twelve code lines.
const OPEN_CAP: f32 = 12.0 * 16.0;

#[test]
fn a_long_command_opens_downward_in_a_capped_row_and_nothing_above_moves() -> Check {
    let mut runtime = runtime("shell/everything")?;
    alone(&mut runtime);
    let key = chat(&runtime)?;
    // The search step in the turn's middle, made the long command a dogfood turn ran.
    runtime.update(|win, _| {
        let Ok(mut hub) = win.hub.try_borrow_mut() else {
            return;
        };
        let shown = yd_ui::shown::thread(&hub, win.window).map(|t| t.id.clone());
        let Some(thread) = shown.and_then(|id| hub.model.wire.threads.get_mut(&id)) else {
            return;
        };
        let tools = &mut thread.transcript.tools;
        let read = tools.get("c1").map(|c| c.output.clone()).unwrap_or_default();
        let printed: Vec<_> = (0..60).flat_map(|_| read.iter().cloned()).collect();
        if let Some(call) = tools.get_mut("c2") {
            let command = "cd /w/lanes/0 && for i in $(seq 1 9); do cargo test -p api upload:: 2>&1 | tail -40; done; ".repeat(12);
            call.name = "bash".to_owned();
            call.args = format!("{{\"command\":{command:?}}}").parse().unwrap_or_default();
            call.output = printed;
        }
    });
    rest(&mut runtime);
    let (read, step) = (format!("{key}.step.c1"), format!("{key}.step.c2"));
    let (first, top) = (rect(&runtime, &read)?[1], rect(&runtime, &step)?[1]);
    tap(&mut runtime, &format!("{step}.verb"))?;
    assert!(live(&runtime, &format!("{step}.detail")), "it opens");
    assert_eq!(rect(&runtime, &step)?[1], top, "its top stays where it was");
    assert_eq!(
        rect(&runtime, &read)?[1],
        first,
        "the row above it did not move"
    );
    let body = rect(&runtime, &format!("{step}.body"))?;
    assert!(
        body[3] <= OPEN_CAP + 0.5,
        "the open row is {} tall",
        body[3]
    );
    let detail = rect(&runtime, &format!("{step}.detail"))?;
    assert!(
        detail[3] <= OPEN_CAP + 40.0,
        "the whole row is {} tall",
        detail[3]
    );
    for _ in 0..3 {
        runtime.frame(1.0 / 60.0);
        assert_eq!(rect(&runtime, &step)?[1], top, "and it stays there");
    }
    Ok(())
}
