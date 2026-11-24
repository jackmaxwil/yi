//! What a user sees and does in the chat pane, drawn in process with no GPU.

use std::error::Error;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use yd_core::ids::WindowId;
use yd_core::pane::PaneKind;
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::{DEF, scenes};

mod composer;

fn root(win: &Win, cx: &mut rune::Cx<Win>) -> rune::El {
    let Ok(hub) = win.hub.try_borrow() else {
        return rune::col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

fn draw(setup: fn(&mut Hub), size: (f32, f32)) -> Result<Runtime<Win>, Box<dyn Error>> {
    static DEFS: [&yd_ui::pane::PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    setup(&mut *hub.try_borrow_mut()?);
    let mut rt = Runtime::new(Win::new(hub, WindowId(0)), root, size, Env::new(true, 1.0));
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok(rt)
}

fn texts(tree: &[ElementInfo]) -> String {
    let words: Vec<&str> = tree.iter().filter_map(|e| e.text.as_deref()).collect();
    words.join("\u{1}")
}

/// Whether a text or an accessible label shows `needle`, as the parity check reads them.
fn has(tree: &[ElementInfo], needle: &str) -> bool {
    let shows = |words: Option<&str>| words.is_some_and(|t| t.contains(needle));
    let live = tree.iter().filter(|e| !e.exiting);
    live.into_iter()
        .any(|e| shows(e.text.as_deref()) || shows(e.label.as_deref()))
}

/// Clicks the first ask's card, so the keys go to it.
fn focus_card(rt: &mut Runtime<Win>, ask: &str) -> Result<(), Box<dyn Error>> {
    let key = format!("chat-0.asks.row.{ask}");
    let tree = rt.elements();
    let card = tree
        .iter()
        .find(|e| e.key.as_deref() == Some(key.as_str()))
        .ok_or("no card")?;
    let [x, y, _, _] = card.rect;
    rt.pointer_down(x + 4.0, y + 4.0);
    rt.pointer_up(x + 4.0, y + 4.0);
    rt.frame(0.016);
    Ok(())
}

fn key(rt: &mut Runtime<Win>, key: Key, cmd: bool) {
    let mods = Mods {
        cmd,
        logo: cmd,
        ..Mods::default()
    };
    rt.key(KeyEvent { key, mods });
    rt.frame(0.016);
}

fn answers(rt: &Runtime<Win>) -> Result<Vec<Option<String>>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    Ok(hub
        .model
        .wire
        .asks
        .iter()
        .map(|a| a.answer.clone())
        .collect())
}

#[test]
fn the_mockup_conversation_reads_as_the_record_says() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::mockup, (336.0, 750.0))?;
    let tree = rt.elements();
    for line in [
        "you · 17:57:58 · 3 files in context",
        "84 lines",
        "3 matches",
        "+6 −1",
        "Opus 5.5 · 17:58:01 · 3.4 s · 612 tokens · 2 files changed",
        "writing · 38 tokens a second · held for your approval",
        "0:42",
        "1 of 2",
        "in ~/atlas-api · sandbox on · no network · writes nothing · about 20 s",
        "Reply, @ a file, / a command",
        "Opus 5.5 · High",
    ] {
        assert!(
            has(&tree, line),
            "{line:?} is not drawn in: {}",
            texts(&tree)
        );
    }
    let edit = tree.iter().any(|e| e.text.as_deref() == Some("Edit"));
    assert!(!edit, "Edit is drawn though the server has no such option");
    Ok(())
}

#[test]
fn the_card_is_three_rows_91_tall_at_320() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::mockup, (336.0, 750.0))?;
    let tree = rt.elements();
    let card = tree
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-0.asks.row.perm-1"))
        .ok_or("no card for the first ask")?;
    assert!(
        (card.rect[2] - 320.0).abs() <= 1.0,
        "card width {}",
        card.rect[2]
    );
    // The record's 91 holds a one-line scope; a scope too long for it wraps rather than cut.
    let scope = tree.iter().find(|e| {
        let said = e.text.as_deref().unwrap_or_default();
        !e.exiting && said.starts_with("in ~/atlas-api")
    });
    let more = scope.map_or(0.0, |scope| scope.rect[3] - 13.0);
    assert!(
        (card.rect[3] - more - 91.0).abs() <= 1.0,
        "card height {} less {more} of wrapped scope (record 3.5.3: 91 in a pane)",
        card.rect[3]
    );
    Ok(())
}

#[test]
fn return_runs_the_shown_ask_and_the_next_one_shows() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::mockup, (336.0, 750.0))?;
    focus_card(&mut rt, "perm-1")?;
    key(&mut rt, Key::Enter, false);
    assert_eq!(answers(&rt)?, [Some("allow_once".to_owned()), None]);
    let tree = rt.elements();
    let next = tree
        .iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some("chat-0.asks.row.perm-2"));
    assert!(
        next && !has(&tree, "1 of 2"),
        "the queue does not move on: {}",
        texts(&tree)
    );
    Ok(())
}

#[test]
fn cmd_return_always_allows_and_escape_denies() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::mockup, (336.0, 750.0))?;
    focus_card(&mut rt, "perm-1")?;
    key(&mut rt, Key::Enter, true);
    focus_card(&mut rt, "perm-2")?;
    key(&mut rt, Key::Escape, false);
    let want = [
        Some("allow_always".to_owned()),
        Some("reject_once".to_owned()),
    ];
    assert_eq!(answers(&rt)?, want);
    Ok(())
}

#[test]
fn the_queue_shows_its_second_ask() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::queue, (336.0, 750.0))?;
    assert!(has(&rt.elements(), "2 of 2"), "the second ask is not shown");
    focus_card(&mut rt, "perm-2")?;
    key(&mut rt, Key::Enter, false);
    assert_eq!(answers(&rt)?, [None, Some("allow_once".to_owned())]);
    Ok(())
}

#[test]
fn an_allowed_ask_folds_to_a_receipt_under_its_step() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::allowed, (600.0, 600.0))?;
    let tree = rt.elements();
    assert!(has(&tree, "echo one > one.txt"), "the step is missing");
    assert!(has(&tree, "Allowed"), "no receipt: {}", texts(&tree));
    assert!(
        !has(&tree, "Run a command"),
        "the answered card is still drawn"
    );
    Ok(())
}

#[test]
fn a_streaming_answer_says_it_is_writing() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::streaming, (600.0, 600.0))?;
    assert!(has(&rt.elements(), "writing"), "no writing line");
    Ok(())
}

#[test]
fn an_empty_pane_still_offers_the_composer() -> Result<(), Box<dyn Error>> {
    let rt = draw(|_| {}, (336.0, 600.0))?;
    assert!(has(&rt.elements(), "Reply, @ a file, / a command"));
    let _ = PaneKind::Chat;
    Ok(())
}

/// A thread with one ask whose options go past the three this app has commands for.
fn wide_ask(hub: &mut Hub) {
    scenes::streaming(hub);
    let Some(thread) = hub.model.wire.listed().first().map(|t| t.id.clone()) else {
        return;
    };
    let option = |id: &str, name: &str, kind: &str| {
        format!(r#"{{"optionId":"{id}","name":"{name}","kind":"{kind}"}}"#)
    };
    let options = [
        option("allow_once", "Allow once", "allow_once"),
        option("never", "Never for this tool", "reject_always"),
        option("ask-lead", "Ask the lead", "escalate"),
    ];
    let ask = format!(
        r#"{{"jsonrpc":"2.0","id":"perm-wide","method":"session/request_permission","params":{{"sessionId":"{thread}","title":"deploy requires permission","options":[{}]}}}}"#,
        options.join(",")
    );
    let line = yd_core::wire::event::WireEvent::Line(ask);
    yd_core::msg::step(&mut hub.model, yd_core::msg::Msg::Wire(line));
}

#[test]
fn every_option_the_server_offers_has_a_button_that_answers_with_it() -> Result<(), Box<dyn Error>>
{
    let mut rt = draw(wide_ask, (336.0, 750.0))?;
    let tree = rt.elements();
    for word in ["Allow once", "Never for this tool", "Ask the lead"] {
        assert!(
            has(&tree, word),
            "no button says {word:?}: {}",
            texts(&tree)
        );
    }
    let key = "chat-0.asks.row.perm-wide.option.ask-lead";
    let button = tree.iter().find(|e| e.key.as_deref() == Some(key));
    let [x, y, w, h] = button
        .ok_or("an option of a kind this app lacks has no button")?
        .rect;
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    assert_eq!(answers(&rt)?, [Some("ask-lead".to_owned())]);
    Ok(())
}

#[test]
fn pressing_the_file_a_step_names_opens_it_in_the_code_viewer() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::mockup, (336.0, 750.0))?;
    let tree = rt.elements();
    let named = |e: &&ElementInfo| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.contains(".step.") && key.ends_with(".path")
    };
    let step = tree.iter().find(named).ok_or("no step names a file")?;
    let (path, [x, y, w, h]) = (step.text.clone().ok_or("no path")?, step.rect);
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    let hub = rt.state.hub.try_borrow()?;
    let open = hub.model.tables.get(yd_core::feed::OPEN_FILES);
    let tab = open.and_then(|open| open.find(&yd_core::ids::RowId::new(format!("file:{path}"))));
    assert!(tab.is_some(), "{path} has no tab in the code viewer");
    Ok(())
}

#[test]
fn a_tap_focuses_the_composer_and_what_is_typed_shows_and_sends() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::queue, (336.0, 750.0))?;
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = std::rc::Rc::clone(&sent);
    rt.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let yd_core::msg::Effect::Wire(request) = effect {
            log.borrow_mut().push(request.line);
        }
    });
    let tree = rt.elements();
    let field = tree
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-0.composer.input"));
    let [x, y, w, h] = field.ok_or("no composer field")?.rect;
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    rt.frame(0.016);
    key(&mut rt, Key::Text("run the command".into()), false);
    assert!(
        has(&rt.elements(), "run the command"),
        "the words typed show"
    );
    key(&mut rt, Key::Enter, false);
    let lines = sent.borrow();
    let prompt =
        |line: &String| line.contains("session/prompt") && line.contains("run the command");
    // The scene's thread waits on an ask, so the prompt waits its turn and says so.
    let queued = has(&rt.elements(), "queued · run the command");
    assert!(lines.iter().any(prompt) || queued, "{lines:?}");
    drop(lines);
    let tree = rt.elements();
    let field = tree
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-0.composer.input"));
    let words = field.and_then(|e| e.text.as_deref());
    assert_eq!(words, Some(""), "and the box is empty again");
    Ok(())
}

/// Two thousand turns: the pane opens at the newest answer with the fold said above, and a
/// reader who scrolls up is offered the way back.
#[test]
fn a_two_thousand_turn_thread_opens_at_its_tail_and_scrolls() -> Result<(), Box<dyn Error>> {
    let built = std::time::Instant::now();
    let mut rt = draw(|hub| scenes::long(hub, 2_000), (336.0, 700.0))?;
    let opened = built.elapsed().as_secs_f64() * 1000.0;
    println!("2,000 turns: built and first drawn in {opened:.1} ms");
    let tree = rt.elements();
    assert!(
        has(&tree, "[…] 50 of 1999 earlier turns folded here"),
        "the fold is not said"
    );
    let view = tree
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-0.conversation"))
        .ok_or("no conversation")?
        .rect;
    let newest = tree
        .iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some("Answer 1999"))
        .ok_or("the newest answer is not drawn")?
        .rect;
    assert!(
        newest[1] >= view[1] && newest[1] + newest[3] <= view[1] + view[3],
        "the newest answer is out of view: {newest:?} in {view:?}"
    );
    rt.wheel_xy(100.0, 200.0, 0.0, 2_000.0);
    for _ in 0..8 {
        rt.frame(0.016);
    }
    assert!(
        has(&rt.elements(), "Jump to latest"),
        "scrolled up, no way back"
    );
    Ok(())
}

/// Pick mode and the right-click menu ask the pane what an element stands for.
fn found(rt: &Runtime<Win>, key: &str) -> Result<Option<(String, String)>, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    let thing = yd_ui::overlay::pick::thing_at(&hub, key);
    Ok(thing.map(|thing| (thing.address.to_string(), thing.label)))
}

/// A recorded turn from a real server: its messages are stored entries, as after a resume.
fn recorded(hub: &mut Hub) {
    let turn = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
    let _ = yd_core::wire::replay::play(&mut hub.model, turn);
}

#[test]
fn a_prompt_an_answer_and_a_tool_step_are_things_pick_mode_can_attach() -> Result<(), Box<dyn Error>>
{
    let rt = draw(recorded, (336.0, 750.0))?;
    let tree = rt.elements();
    let keyed = |ends: &str| {
        let keys = tree.iter().filter_map(|e| e.key.as_deref());
        let hit = keys
            .filter(|key| key.ends_with(ends))
            .map(str::to_owned)
            .next();
        hit.ok_or_else(|| format!("nothing keyed {ends} in {}", texts(&tree)))
    };
    let (bubble, answer) = (keyed(".bubble")?, keyed(".answer")?);
    let step = tree.iter().filter_map(|e| e.key.as_deref());
    let step = step.filter(|key| key.contains(".step.") && !key.ends_with(".path"));
    let step = step.map(str::to_owned).next().ok_or("no step drawn")?;
    let (prompt, _) = found(&rt, &bubble)?.ok_or("a prompt is no thing")?;
    assert!(prompt.starts_with("yi://message/"), "{prompt}");
    let (said, _) = found(&rt, &answer)?.ok_or("an answer is no thing")?;
    assert!(said.starts_with("yi://answer/"), "{said}");
    let (ran, label) = found(&rt, &step)?.ok_or("a step is no thing")?;
    let call = step
        .rsplit_once(".step.")
        .map(|(_, id)| id)
        .unwrap_or_default();
    assert!(
        ran.starts_with("yi://step/") && ran.ends_with(call),
        "{ran}"
    );
    assert!(label.starts_with("step · "), "{label}");
    assert_eq!(
        found(&rt, "chat-0.composer")?,
        None,
        "the composer is no thing"
    );
    Ok(())
}

/// Where the command and the first button of the first ask's card are drawn.
fn command_and_run(rt: &Runtime<Win>) -> Result<([f32; 4], [f32; 4]), Box<dyn Error>> {
    let tree = rt.elements();
    let rect = |key: &str| {
        let hit = tree.iter().find(|e| e.key.as_deref() == Some(key));
        hit.map(|e| e.rect).ok_or_else(|| format!("no {key}"))
    };
    let command = rect("chat-0.asks.row.perm-1.command")?;
    Ok((command, rect("chat-0.asks.row.perm-1.approval.allow")?))
}

/// The mockup's rule (its `flex-wrap` row): the buttons sit beside the command while both
/// fit, and under it in a narrow pane; the command never squeezes below 150.
#[test]
fn the_cards_buttons_sit_beside_the_command_only_where_both_fit() -> Result<(), Box<dyn Error>> {
    let (command, run) = command_and_run(&draw(scenes::mockup, (336.0, 750.0))?)?;
    assert!(
        run[1] > command[1] + command[3] - 1.0,
        "narrow: {command:?} {run:?}"
    );
    assert!(
        (command[2] - 302.0).abs() <= 1.0,
        "the command fills its line: {command:?}"
    );
    let (command, run) = command_and_run(&draw(scenes::mockup, (720.0, 750.0))?)?;
    let beside = run[0] >= command[0] + command[2] && run[1] < command[1] + command[3];
    assert!(beside, "wide: command {command:?}, Run {run:?}");
    assert!(command[2] >= 150.0, "{command:?}");
    Ok(())
}

/// Record 3.4.3: tool rows are 20 high, one under the next; the conversation's 8 point gap
/// is between what is said, not between the steps of one piece of work.
#[test]
fn tool_rows_sit_one_under_the_next_with_no_gap() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::mockup, (336.0, 750.0))?;
    let tree = rt.elements();
    let step = |id: &str| {
        let key = format!("chat-0.step.{id}");
        let hit = tree.iter().find(|e| e.key.as_deref() == Some(key.as_str()));
        hit.map(|e| e.rect).ok_or(format!("no {key}"))
    };
    let tops = [
        step("c1")?[1],
        step("c2")?[1],
        step("c3")?[1],
        step("c4")?[1],
    ];
    for pair in tops.windows(2) {
        let apart = pair.get(1).zip(pair.first()).map(|(b, a)| b - a);
        assert_eq!(apart, Some(20.0), "{tops:?}");
    }
    Ok(())
}

/// The words drawn inside the element keyed `key`, top to bottom and left to right.
fn inside(tree: &[ElementInfo], key: &str) -> Result<String, Box<dyn Error>> {
    let live = tree.iter().filter(|e| !e.exiting);
    let mut live = live.collect::<Vec<_>>();
    let outer = live.iter().find(|e| e.key.as_deref() == Some(key));
    let [x, y, w, h] = outer
        .ok_or_else(|| format!("no {key} in {}", texts(tree)))?
        .rect;
    let within = |r: [f32; 4]| {
        r[0] >= x - 0.5 && r[1] >= y - 0.5 && r[0] < x + w && r[1] + r[3] <= y + h + 0.5
    };
    live.retain(|e| e.text.is_some() && within(e.rect));
    live.sort_by(|a, b| {
        (a.rect[1], a.rect[0])
            .partial_cmp(&(b.rect[1], b.rect[0]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let words: Vec<&str> = live.iter().filter_map(|e| e.text.as_deref()).collect();
    Ok(words.join(" "))
}

fn press(rt: &mut Runtime<Win>, key: &str) -> Result<(), Box<dyn Error>> {
    let tree = rt.elements();
    let hit = tree
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    let [x, y, w, h] = hit
        .ok_or_else(|| format!("no {key} in {}", texts(&tree)))?
        .rect;
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    rt.pointer_down(x + w / 2.0, y + h / 2.0);
    rt.pointer_up(x + w / 2.0, y + h / 2.0);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    Ok(())
}

#[test]
fn a_finished_turn_folds_to_what_it_asked_and_opens_on_a_press() -> Result<(), Box<dyn Error>> {
    let mut rt = draw(scenes::dogfood, (336.0, 750.0))?;
    let folded = inside(&rt.elements(), "chat-0.turn.1")?;
    assert!(
        folded.contains("What does this project do?") && folded.contains("26.0 s"),
        "the fold says {folded:?}"
    );
    assert!(
        !has(&rt.elements(), "Tally sums CSV text."),
        "folded, it hides the answer"
    );
    press(&mut rt, "chat-0.turn.1")?;
    assert!(
        has(&rt.elements(), "Tally sums CSV text."),
        "a press opens the turn"
    );
    Ok(())
}

#[test]
fn a_step_held_by_its_ask_says_it_waits_for_you() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::dogfood, (336.0, 750.0))?;
    let step = inside(&rt.elements(), "chat-0.step.e1")?;
    assert!(
        step.contains("waiting for you")
            && step.contains("src/main.rs")
            && !step.contains("running"),
        "{step:?}"
    );
    Ok(())
}

#[test]
fn the_edit_card_names_the_file_it_changes_and_by_how_much() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::dogfood, (336.0, 750.0))?;
    let well = inside(&rt.elements(), "chat-0.asks.row.perm-e1.command")?;
    assert!(well == "src/main.rs +2 −1", "the card says {well:?}");
    Ok(())
}

#[test]
fn a_long_command_and_its_folder_are_drawn_whole() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::commanding, (270.0, 750.0))?;
    let tree = rt.elements();
    let well = by(&tree, "chat-0.asks.row.perm-b1.command")?;
    let whole = format!("$ {}", scenes::LONG);
    let mut words = tree
        .iter()
        .filter(|e| e.text.as_deref() == Some(whole.as_str()));
    let text = words.next().ok_or("the whole command is not drawn")?;
    assert!(
        text.rect[3] > 30.0 && text.rect[0] + text.rect[2] <= well[0] + well[2] + 0.5,
        "it wraps inside the well: {:?} in {well:?}",
        text.rect
    );
    let scope = tree.iter().find(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("in /w/tally"))
    });
    let scope = scope.ok_or("no folder line")?;
    assert!(scope.rect[0] + scope.rect[2] <= 270.0, "{:?}", scope.rect);
    Ok(())
}

fn by(tree: &[ElementInfo], key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let hit = tree
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    Ok(hit.ok_or_else(|| format!("no {key}"))?.rect)
}

/// The eight-pane layout gives chat about 270 points: nothing a person presses is cut.
#[test]
fn no_button_of_the_card_or_the_composer_is_cut_in_a_narrow_pane() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::dogfood, (270.0, 750.0))?;
    let tree = rt.elements();
    let card = "chat-0.asks.row.perm-e1.";
    let pressed = tree.iter().filter(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting
            && (key.starts_with(card) && !key.ends_with(".command")
                || key == "chat-0.composer.send")
    });
    let mut seen = 0;
    for e in pressed {
        seen += 1;
        assert!(
            e.rect[0] + e.rect[2] <= 270.0 + 0.5,
            "{:?} is cut: {:?}",
            e.key,
            e.rect
        );
    }
    assert!(seen >= 4, "the three answers and send are drawn");
    Ok(())
}

#[test]
fn a_stopped_turn_says_so_and_its_command_says_why_it_ended() -> Result<(), Box<dyn Error>> {
    let rt = draw(scenes::stopped, (336.0, 750.0))?;
    let tree = rt.elements();
    assert!(
        has(&tree, "Stopped"),
        "no trace of the stop: {}",
        texts(&tree)
    );
    let step = inside(&tree, "chat-0.step.b1")?;
    assert!(
        step.contains("stopped with the turn") && !step.contains("[command aborted]"),
        "{step:?}"
    );
    Ok(())
}

/// A new app on the server of `resume.jsonl`, whose ask was asked before it started.
fn restarted(hub: &mut Hub) {
    let lines: Vec<&str> = include_str!("../../../../../fixtures/wire/resume.jsonl")
        .lines()
        .collect();
    let replayed = lines
        .get(28)
        .into_iter()
        .chain(lines.get(29..34).unwrap_or_default());
    let _ = yd_core::wire::replay::play(
        &mut hub.model,
        &replayed.copied().collect::<Vec<_>>().join("\n"),
    );
}

#[test]
fn a_lost_approval_says_yi_still_waits_and_a_prompt_says_why_it_is_not_sent()
-> Result<(), Box<dyn Error>> {
    let mut rt = draw(restarted, (270.0, 750.0))?;
    let tree = rt.elements();
    let lost = tree.iter().find(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.starts_with("This approval was lost when the app quit"))
    });
    let lost = lost.ok_or_else(|| format!("no lost line in {}", texts(&tree)))?;
    assert!(
        lost.rect[3] > 14.0,
        "the reason wraps, it is not cut: {:?}",
        lost.rect
    );
    press(&mut rt, "chat-0.composer.input")?;
    key(&mut rt, Key::Text("Please continue.".into()), false);
    key(&mut rt, Key::Enter, false);
    let tree = rt.elements();
    assert!(
        has(
            &tree,
            "not sent, a lost approval holds the run · Please continue."
        ),
        "a prompt to a held run reads as queued: {}",
        texts(&tree)
    );
    Ok(())
}

/// The keycap drawn inside the button keyed `key`, if any.
fn cap_in(tree: &[ElementInfo], key: &str) -> Option<String> {
    let at = tree.iter().position(|e| e.key.as_deref() == Some(key))?;
    let under = |e: &&ElementInfo| {
        std::iter::successors(e.parent, |p| tree.get(*p).and_then(|e| e.parent)).any(|p| p == at)
    };
    let caps = tree
        .iter()
        .filter(under)
        .filter(|e| e.role == yd_ui::kit::key_hint::KEYCAP);
    let cap = caps.into_iter().next()?;
    let index = tree.iter().position(|e| std::ptr::eq(e, cap))?;
    let text = tree
        .iter()
        .find(|e| e.parent == Some(index) && e.text.is_some());
    text.and_then(|e| e.text.clone())
}

#[test]
fn cmd_return_allows_yis_narrowest_scope_and_never_the_whole_tree() -> Result<(), Box<dyn Error>> {
    for (setup, want) in [
        (scenes::dogfood as fn(&mut Hub), "allow_always"),
        (scenes::at_root, "allow_always"),
    ] {
        let mut rt = draw(setup, (336.0, 750.0))?;
        let tree = rt.elements();
        let marked = "chat-0.asks.row.perm-e1.approval.always";
        assert_eq!(cap_in(&tree, marked).as_deref(), Some("⌘↵"), "{want}");
        let other = format!("chat-0.asks.row.perm-e1.option.{}", "allow_always_1");
        assert!(
            by(&tree, &other).is_ok() && cap_in(&tree, &other).is_none(),
            "{other}"
        );
        focus_card(&mut rt, "perm-e1")?;
        key(&mut rt, Key::Enter, true);
        assert_eq!(answers(&rt)?, [Some(want.to_owned())]);
    }
    Ok(())
}

#[test]
fn the_widest_always_button_wraps_inside_a_narrow_card() -> Result<(), Box<dyn Error>> {
    for width in [160.0, 270.0] {
        let rt = draw(scenes::at_root, (width, 750.0))?;
        let tree = rt.elements();
        let words = "Always allow edits anywhere in this tree";
        let label = tree
            .iter()
            .find(|e| !e.exiting && e.text.as_deref() == Some(words));
        let [x, _, w, _] = label.ok_or("the widest answer is not drawn")?.rect;
        assert!(x + w <= width + 0.5, "cut at {width}: {x} + {w}");
    }
    Ok(())
}
