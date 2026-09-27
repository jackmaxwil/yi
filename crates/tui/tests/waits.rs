//! What the turn is held up by, and the call the model is still writing, drawn as they happen.
mod common;

use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use yi_orb::OrbState;
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_types::event::{AgentEvent, AssistantMessageEvent, Wait};

type TestResult = Result<(), Box<dyn Error>>;

fn running() -> App {
    let mut app = App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "waits".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        yi_tui::keymap::default_keymap(),
        80,
    );
    app.reduce_agent(AgentEvent::AgentStart);
    app
}

fn screen(app: &mut App) -> String {
    let area = Rect::new(0, 0, 80, 20);
    let mut buffer = Buffer::empty(area);
    let _ = yi_tui::render::paint_pane(app, None, &mut buffer, area, &mut 0);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn update(event: AssistantMessageEvent) -> AgentEvent {
    AgentEvent::MessageUpdate {
        assistant_message_event: event,
    }
}

/// Dies with the waits unnamed: a provider's backoff, a compaction and a kernel boot all read
/// "Working…" or "Waiting…", however long they held the turn.
#[test]
fn each_wait_names_its_cause_and_puts_its_own_state_on_the_orb() -> TestResult {
    let mut app = running();
    app.reduce_agent(AgentEvent::Wait {
        wait: Some(Wait::Retry {
            attempt: 1,
            of: 3,
            delay_ms: 6_000,
            cause: "HTTP 429".to_owned(),
        }),
    });
    assert_eq!(app.orb_state(), Some(OrbState::Stalled));
    let shown = screen(&mut app);
    assert!(
        shown.contains("Retrying · faux HTTP 429 · 1 of 3 in 6 s"),
        "{shown}"
    );
    app.reduce_agent(update(AssistantMessageEvent::TextStart {
        content_index: 0,
    }));
    assert_ne!(
        app.orb_state(),
        Some(OrbState::Stalled),
        "the stream resumed"
    );
    app.reduce_agent(AgentEvent::Wait {
        wait: Some(Wait::Compaction { tokens: 103_212 }),
    });
    assert_eq!(app.orb_state(), Some(OrbState::Condensing));
    let shown = screen(&mut app);
    assert!(shown.contains("Condensing · 103K tokens · 0 s"), "{shown}");
    app.reduce_agent(AgentEvent::Wait {
        wait: Some(Wait::KernelBoot {
            step: "installing the kernel's packages".to_owned(),
        }),
    });
    assert_eq!(app.orb_state(), Some(OrbState::KernelBoot));
    let shown = screen(&mut app);
    assert!(
        shown.contains("Starting the kernel · installing the kernel's packages"),
        "{shown}"
    );
    app.reduce_agent(AgentEvent::Wait { wait: None });
    assert_ne!(app.orb_state(), Some(OrbState::KernelBoot));
    Ok(())
}

/// Dies with the arguments dropped until the call ended: a file the model spent 87 s writing
/// drew nothing until it was whole.
#[test]
fn a_tool_call_draws_while_its_arguments_stream() -> TestResult {
    let mut app = running();
    app.reduce_agent(update(AssistantMessageEvent::ToolCallStart {
        content_index: 0,
        name: Some("write".to_owned()),
    }));
    app.reduce_agent(update(AssistantMessageEvent::ToolCallDelta {
        content_index: 0,
        delta: r##"{"path": "docs/plan.md", "content": "# Plan\nfirst step\nsecond st"##.to_owned(),
    }));
    assert_eq!(app.orb_state(), Some(OrbState::Editing));
    let shown = screen(&mut app);
    for needle in ["docs/plan.md", "first step", "second st", "3 lines so far"] {
        assert!(shown.contains(needle), "lacks {needle:?}:\n{shown}");
    }
    Ok(())
}
