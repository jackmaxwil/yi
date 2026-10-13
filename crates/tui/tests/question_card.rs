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
        80,
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
        first.contains("(recommended)") && first.trim_end().ends_with('1'),
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
