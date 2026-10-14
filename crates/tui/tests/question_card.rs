//! A root's `ask_user` call as the person sees it: the card, its keys, the one reply it sends.
use crate::common;

use std::error::Error;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use yi_tui::app::{App, Command, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::AgentEvent;

type TestResult = Result<(), Box<dyn Error>>;

fn app() -> App {
    app_at(80)
}

fn app_at(width: usize) -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "question".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        width,
    )
}

/// One turn in which the model called `ask_user` once per entry of `calls`, then ended.
fn asked(app: &mut App, calls: &[Value]) {
    app.reduce_agent(AgentEvent::AgentStart);
    for (at, args) in calls.iter().enumerate() {
        app.reduce_agent(AgentEvent::ToolExecutionStart {
            tool_call_id: format!("ask-{at}"),
            tool_name: "ask_user".to_owned(),
            args: args.clone(),
        });
    }
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
}

fn press(app: &mut App, codes: &[KeyCode]) -> Vec<String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for code in codes {
        let event = Event::Key(KeyEvent::new(*code, KeyModifiers::NONE));
        yi_tui::input::handle_terminal_event(app, &tx, event);
    }
    let mut sent = Vec::new();
    while let Ok(command) = rx.try_recv() {
        sent.push(match command {
            Command::Prompt(text) => text,
            Command::Steer(text) => format!("steer {text}"),
            _ => "other".to_owned(),
        });
    }
    sent
}

fn keys(text: &str) -> Vec<KeyCode> {
    text.chars().map(KeyCode::Char).collect()
}

fn layout() -> Value {
    json!({
        "question": "Which layout for the plan pane?",
        "options": [
            {"label": "Tree with a progress bar", "description": "Keeps nested todos as they are today."},
            {"label": "Stages", "description": "Numbers each todo by how long its chain is.",
             "preview": "stage 1  read\nstage 2  edit\nstage 3  test"},
            {"label": "Focus card"}
        ]
    })
}

fn row_with(rows: &[String], needle: &str) -> Option<String> {
    rows.iter().find(|row| row.contains(needle)).cloned()
}

/// Dies with the question reading as prose: a root's `ask_user` left only a tool row and the
/// options in the model's reply, with no recommended mark, trade-off line, key or Other.
#[test]
fn a_question_opens_as_a_card_with_the_recommendation_first() -> TestResult {
    let mut app = app();
    asked(&mut app, &[layout()]);
    let rows = common::live_rows(&mut app)?;
    let first = row_with(&rows, "Tree with a progress bar").ok_or(format!("{rows:#?}"))?;
    assert!(
        first.contains("(Recommended)") && first.trim_end().ends_with('1'),
        "{first:?}"
    );
    assert!(row_with(&rows, "Keeps nested todos").is_some(), "{rows:#?}");
    let other = row_with(&rows, "Other").ok_or(format!("{rows:#?}"))?;
    assert!(other.trim_end().ends_with('4'), "{other:?}");
    Ok(())
}

/// Dies with the pick sent as a numeral: the reply the model reads is the option's words.
#[test]
fn a_number_key_answers_with_the_option_label() -> TestResult {
    let mut app = app();
    asked(&mut app, &[layout()]);
    assert_eq!(press(&mut app, &keys("2")), ["Stages"]);
    let rows = common::live_rows(&mut app)?;
    assert!(
        row_with(&rows, "Which layout").is_none(),
        "the card closes on the pick: {rows:#?}"
    );
    Ok(())
}

/// Dies with Other swallowing digits and `s` as picks: typing into it must stay typing.
#[test]
fn other_takes_the_users_own_words_digits_and_s_included() -> TestResult {
    let mut app = app();
    asked(&mut app, &[layout()]);
    let mut script = keys("4");
    script.extend(keys("sort by 2 phases"));
    script.push(KeyCode::Enter);
    assert_eq!(press(&mut app, &script), ["sort by 2 phases"]);
    Ok(())
}

/// Dies with two questions asked in one turn arriving as two cards, or as only the last: a
/// page's answer must wait for the rest and go out as one message.
#[test]
fn questions_asked_in_one_turn_are_pages_of_one_reply() -> TestResult {
    let mut app = app();
    let second = json!({
        "question": "Which look?",
        "options": ["Quiet", "Loud"]
    });
    asked(&mut app, &[layout(), second]);
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "1/2").is_some(), "{rows:#?}");
    assert_eq!(press(&mut app, &keys("1")), Vec::<String>::new());
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "2/2").is_some(), "{rows:#?}");
    assert_eq!(
        press(&mut app, &keys("2")),
        ["Which layout for the plan pane? → Tree with a progress bar\nWhich look? → Loud"]
    );
    Ok(())
}

/// Dies with a skip taken for an answer of nothing: the model must be told to go on option 1.
#[test]
fn skip_tells_the_model_to_take_its_own_recommendation() -> TestResult {
    let mut app = app();
    asked(&mut app, &[layout()]);
    let sent = press(&mut app, &keys("s"));
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent.first()
            .is_some_and(|text| text.contains("recommendation"))
    );
    Ok(())
}

/// Dies with a preview drawn for every option, or never: only the highlighted one shows, and a
/// cut one says how much it kept.
#[test]
fn the_highlighted_option_shows_its_preview_and_names_a_cut() -> TestResult {
    let mut app = app();
    let long = (1..=9)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let args = json!({
        "question": "Which mock?",
        "options": [{"label": "A", "preview": "only a"}, {"label": "B", "preview": long}]
    });
    asked(&mut app, &[args]);
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "only a").is_some(), "{rows:#?}");
    assert!(row_with(&rows, "line 1").is_none(), "{rows:#?}");
    press(&mut app, &[KeyCode::Down]);
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "line 5").is_some(), "{rows:#?}");
    assert!(row_with(&rows, "line 6").is_none(), "{rows:#?}");
    let cut = row_with(&rows, "showing 5 of 9").ok_or(format!("{rows:#?}"))?;
    assert!(cut.contains("at most 5"), "{cut:?}");
    Ok(())
}

/// Dies with a fifth question dropped without a word: the card must page every question the
/// turn asked, or the person answers four and the model waits on a fifth it never shows.
#[test]
fn five_questions_are_five_pages() -> TestResult {
    let mut app = app();
    let calls: Vec<Value> = (1..=5)
        .map(|n| json!({"question": format!("Question {n}?"), "options": ["Yes", "No"]}))
        .collect();
    asked(&mut app, &calls);
    for _ in 0..4 {
        assert_eq!(press(&mut app, &keys("1")), Vec::<String>::new());
    }
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "5/5").is_some(), "{rows:#?}");
    let sent = press(&mut app, &keys("2"));
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent.first()
            .is_some_and(|text| text.ends_with("Question 5? → No"))
    );
    Ok(())
}

/// Dies with an open question left as a bare tool row: with no option the card is the answer
/// field alone and takes typing at once.
#[test]
fn a_question_with_no_options_takes_typing_at_once() -> TestResult {
    let mut app = app();
    asked(
        &mut app,
        &[json!({"question": "What should the lane be called?"})],
    );
    let rows = common::live_rows(&mut app)?;
    assert!(
        row_with(&rows, "What should the lane").is_some(),
        "{rows:#?}"
    );
    assert!(row_with(&rows, "Answer").is_some(), "{rows:#?}");
    let mut script = keys("auth-fix");
    script.push(KeyCode::Enter);
    assert_eq!(press(&mut app, &script), ["auth-fix"]);
    Ok(())
}

/// Dies with the limit and one past it treated alike: five preview lines show whole, six name
/// the cut. Bare-string options (a child's form) read as labels.
#[test]
fn a_five_line_preview_is_whole_and_bare_string_options_read() -> TestResult {
    let mut app = app();
    let five = (1..=5)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    asked(
        &mut app,
        &[json!({"question": "Which?", "options": [{"label": "A", "preview": five}, "B"]})],
    );
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "line 5").is_some(), "{rows:#?}");
    assert!(row_with(&rows, "showing").is_none(), "{rows:#?}");
    assert!(row_with(&rows, " B ").is_some(), "{rows:#?}");
    let mut cut = app_at(80);
    let six = format!("{five}\nline 6");
    asked(
        &mut cut,
        &[json!({"question": "Which?", "options": [{"label": "A", "preview": six}]})],
    );
    let rows = common::live_rows(&mut cut)?;
    assert!(row_with(&rows, "line 6").is_none(), "{rows:#?}");
    assert!(row_with(&rows, "showing 5 of 6").is_some(), "{rows:#?}");
    Ok(())
}

/// Dies with a wide-glyph label overflowing a 40-column screen into the number key.
#[test]
fn wide_glyphs_keep_the_key_on_the_row_at_forty_columns() -> TestResult {
    let mut app = app();
    asked(
        &mut app,
        &[
            json!({"question": "どれにしますか?", "options": ["静かな見た目にする", "賑やかな見た目にする"]}),
        ],
    );
    let rows = common::rows_at(&mut app, 24)?;
    let first = row_with(&rows, "静かな").ok_or(format!("{rows:#?}"))?;
    assert!(
        first.trim_end().ends_with('1') || first.contains('…'),
        "{first:?}"
    );
    Ok(())
}

/// Dies with the answered question left as five rows cut by `… 1 more lines`, or with a label's
/// own colon read as the description's: one row says what was asked, every label whole.
#[test]
fn an_answered_question_leaves_one_row_not_the_models_boilerplate() -> TestResult {
    use yi_types::event::ToolResult;
    use yi_types::message::Content;
    let mut app = app();
    let args = json!({
        "question": "Which layout?",
        "options": [
            {"label": "Tree: nested", "description": "Keeps todos as they are."},
            {"label": "Stages"}
        ]
    });
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: "ask-0".to_owned(),
        tool_name: "ask_user".to_owned(),
        args,
    });
    let text = "Question for the user: Which layout?\n  1. Tree: nested: Keeps todos as they are.\n  2. Stages\nThe turn ends here; the user's next message is the answer.";
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: "ask-0".to_owned(),
        tool_name: "ask_user".to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details: Value::Null,
            usage: None,
            added_tool_names: None,
            terminate: Some(true),
        },
        is_error: false,
    });
    let rows: Vec<String> = app
        .take_commits()
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();
    let asked = row_with(&rows, "Which layout").ok_or(format!("{rows:#?}"))?;
    assert!(
        asked.contains("Which layout?  [Tree: nested · Stages]"),
        "{asked:?}"
    );
    assert!(row_with(&rows, "The turn ends here").is_none(), "{rows:#?}");
    assert!(row_with(&rows, "more line").is_none(), "{rows:#?}");
    Ok(())
}

/// Dies with the card drawn as text alone on a kitty terminal, its image left on screen after
/// it closes, or a repaint every frame: one image under the text, repainted only on a change.
#[test]
fn a_kitty_terminal_gets_the_card_as_one_image_under_its_text() -> TestResult {
    use yi_tui::question::{CardTick, tick_with};
    let mut app = app();
    app.set_kitty(true);
    asked(&mut app, &[layout()]);
    common::rows_at(&mut app, 34)?;
    let (mut state, mut out) = (CardTick::default(), Vec::new());
    tick_with(&app, &mut out, &mut state, (10, 20));
    let sent = String::from_utf8_lossy(&out).into_owned();
    assert!(
        sent.contains("a=t,i=769"),
        "no image sent: {} bytes",
        sent.len()
    );
    assert_eq!(
        sent.matches("z=-1").count(),
        1,
        "one placement, under the text"
    );
    out.clear();
    tick_with(&app, &mut out, &mut state, (10, 20));
    assert!(out.is_empty(), "an unchanged card sends nothing");
    press(&mut app, &[KeyCode::Down]);
    common::rows_at(&mut app, 34)?;
    tick_with(&app, &mut out, &mut state, (10, 20));
    let moved = String::from_utf8_lossy(&out).into_owned();
    assert!(
        moved.contains("a=t,i=769") && moved.contains("a=d"),
        "{moved:.80}"
    );
    out.clear();
    press(&mut app, &[KeyCode::Esc]);
    common::rows_at(&mut app, 34)?;
    tick_with(&app, &mut out, &mut state, (10, 20));
    let gone = String::from_utf8_lossy(&out).into_owned();
    assert_eq!(
        gone.matches("a=d").count(),
        2,
        "both image ids deleted: {gone}"
    );
    Ok(())
}

/// Dies with a long question cut at the card's edge: the prompt is the card's content, so it
/// wraps onto the rows below and every word of it is on screen.
#[test]
fn a_long_question_wraps_and_keeps_every_word() -> TestResult {
    let mut app = app();
    let prompt = "Should the plan pane keep its tree with a progress bar under every stage, or number each todo by how long its chain is and end on the zebra?";
    asked(
        &mut app,
        &[json!({"question": prompt, "options": ["Tree", "Stages"]})],
    );
    let rows = common::rows_at(&mut app, 34)?;
    let shown = rows.join(" ");
    for word in prompt.split_whitespace() {
        assert!(shown.contains(word), "{word} missing: {rows:#?}");
    }
    assert!(row_with(&rows, "zebra?").is_some(), "{rows:#?}");
    Ok(())
}

/// Dies with the side pane missing at 110 columns and up, or with a long preview's last row or
/// its cut row drawn below the pane, floating on its own just above the footer.
#[test]
fn a_wide_card_keeps_a_long_preview_inside_its_side_pane() -> TestResult {
    let mut app = app_at(120);
    let long = (1..=30)
        .map(|n| format!("mock {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let args = json!({
        "question": "Which mock?",
        "options": [{"label": "Dense", "preview": long}, {"label": "Sparse"}]
    });
    asked(&mut app, &[args]);
    let rows = common::rows_sized(&mut app, 120, 40)?;
    let title = row_with(&rows, "Dense (Recommended)").ok_or(format!("{rows:#?}"))?;
    assert!(
        title.contains("Preview"),
        "the pane sits beside the tiles: {title:?}"
    );
    assert!(row_with(&rows, "mock 1").is_some(), "{rows:#?}");
    let note = rows
        .iter()
        .position(|row| row.contains("of 30 lines; pane fits"))
        .ok_or(format!("{rows:#?}"))?;
    let below = rows
        .get(note.saturating_add(1))
        .ok_or("no row below the cut")?;
    assert!(
        !below.contains("Submit"),
        "the cut row sits on the pane's edge: {rows:#?}"
    );
    press(&mut app, &[KeyCode::Down]);
    let rows = common::rows_sized(&mut app, 120, 40)?;
    assert!(
        row_with(&rows, "No preview for this option").is_some(),
        "{rows:#?}"
    );
    Ok(())
}

/// Dies with the questions dropped when a popup was up at the turn's end: the model waits on
/// an answer the person was never shown. The card opens once the popup closes.
#[test]
fn a_popup_up_at_the_turns_end_holds_the_card_until_it_closes() -> TestResult {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    press(&mut app, &keys("/"));
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: "ask-0".to_owned(),
        tool_name: "ask_user".to_owned(),
        args: layout(),
    });
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let rows = common::live_rows(&mut app)?;
    assert!(row_with(&rows, "Recommended").is_none(), "{rows:#?}");
    press(&mut app, &[KeyCode::Esc]);
    let rows = common::live_rows(&mut app)?;
    assert!(
        row_with(&rows, "Tree with a progress bar").is_some(),
        "{rows:#?}"
    );
    Ok(())
}

/// Dies with one console pane's card erased by another's: a hidden pane deleted the shared ids
/// the focused pane had just placed, and its card lost every panel and tile.
#[test]
fn hiding_one_panes_card_leaves_another_panes_image() -> TestResult {
    use yi_tui::question::{CardTick, hide, tick_with};
    let mut app = app();
    app.set_kitty(true);
    asked(&mut app, &[layout()]);
    common::rows_at(&mut app, 34)?;
    let (mut left, mut right) = (
        CardTick::with_ids([7200, 7201]),
        CardTick::with_ids([7202, 7203]),
    );
    let mut out = Vec::new();
    tick_with(&app, &mut out, &mut left, (10, 20));
    tick_with(&app, &mut out, &mut right, (10, 20));
    out.clear();
    hide(&mut out, &mut left);
    let sent = String::from_utf8_lossy(&out).into_owned();
    assert!(sent.contains("i=7200") && sent.contains("i=7201"), "{sent}");
    assert!(
        !sent.contains("i=7202") && !sent.contains("i=7203"),
        "{sent}"
    );
    Ok(())
}

/// Dies with a moved highlight leaving ink behind or missing a tile edge: the image a key
/// repaints in part must be the image a fresh card paints whole, byte for byte.
#[test]
fn a_partly_repainted_card_is_the_card_painted_whole() -> TestResult {
    use yi_tui::question::{CardTick, tick_with};
    let mut app = app();
    app.set_kitty(true);
    asked(&mut app, &[layout()]);
    common::rows_at(&mut app, 34)?;
    let (mut moved, mut out) = (CardTick::with_ids([1, 2]), Vec::new());
    tick_with(&app, &mut out, &mut moved, (10, 20));
    press(&mut app, &[KeyCode::Down, KeyCode::Down]);
    common::rows_at(&mut app, 34)?;
    out.clear();
    tick_with(&app, &mut out, &mut moved, (10, 20));
    let (mut whole, mut fresh) = (CardTick::with_ids([2, 1]), Vec::new());
    tick_with(&app, &mut fresh, &mut whole, (10, 20));
    let image = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes).into_owned();
        text.split("\u{1b}7")
            .next()
            .map(str::to_owned)
            .unwrap_or_default()
    };
    assert!(image(&fresh).contains("a=t,i=1,"), "{:.80}", image(&fresh));
    assert!(
        image(&out) == image(&fresh),
        "the partial repaint drifted from a whole paint"
    );
    Ok(())
}
