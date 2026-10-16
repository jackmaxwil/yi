use crate::scratch::Scratch;
use crate::todo_mirror::{memory_store, session, wired};

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::ProviderStream;
use yi_runtime::todo::coupling::INTERCEPT_CUSTOM_TYPE;
use yi_types::message::{AgentMessage, Content, StopReason};

type TestResult = Result<(), Box<dyn Error>>;

fn call(id: &str, tool: &str, args: Value) -> Content {
    faux_tool_call(id, tool, args.as_object().cloned().unwrap_or_default())
}

fn calls(batch: Vec<Content>) -> AgentMessage {
    faux_assistant_message(batch, StopReason::ToolUse)
}

fn says(text: &str) -> AgentMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

/// Runs `script` as the model's replies to one prompt in a session wired as `yi` wires one.
async fn run(
    root: &Scratch,
    script: Vec<AgentMessage>,
) -> Result<Vec<AgentMessage>, Box<dyn Error>> {
    run_capped(root, script, None, None).await
}

async fn run_capped(
    root: &Scratch,
    script: Vec<AgentMessage>,
    turns: Option<u32>,
    typed: Option<&str>,
) -> Result<Vec<AgentMessage>, Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(script);
    let mut session = session(Arc::clone(&provider));
    session.attach_store(memory_store())?;
    wired(&mut session, root, provider);
    if let Some(turns) = turns {
        session.set_turn_cap(turns);
    }
    session.prompt("write the note")?;
    if let Some(typed) = typed {
        session.follow_up(typed);
    }
    session.wait_idle().await;
    Ok(session.messages())
}

/// The prose of every assistant message that is not a tool call: what the user saw as answers.
fn answers(messages: &[AgentMessage]) -> Vec<String> {
    (messages.iter())
        .filter_map(|message| match message {
            AgentMessage::Assistant { content, .. }
                if !content
                    .iter()
                    .any(|block| matches!(block, Content::ToolCall { .. })) =>
            {
                Some(yi_types::message::join_text(content, ""))
            }
            _ => None,
        })
        .collect()
}

fn notes(messages: &[AgentMessage], kind: &str) -> usize {
    (messages.iter())
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == kind)
        })
        .count()
}

/// Dies with the draft rewritten whole or shown before it passed: the refused answer must reach
/// the user once, as patched by `edit`, and the model's next reply must never be asked for.
#[tokio::test]
async fn a_refused_answer_is_patched_with_edit_and_shown_once() -> TestResult {
    let root = Scratch::new("yi-answer-patched")?;
    let held = (root.join("rlm/family").join(yi_tools::SPILLS)).join("answers/draft.md");
    let held = held.display();
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        calls(vec![call(
            "c2",
            "answer",
            json!({"summary": "Wrote the note", "text": "Wrote the note.\nThe check is pending."}),
        )]),
        calls(vec![call(
            "c3",
            "todo",
            json!({"op": "done", "label": "write the note", "evidence": "`true` exit 0"}),
        )]),
        calls(vec![call(
            "c4",
            "edit",
            json!({"patch": format!("[{held}]\nPUT 2.=2:\n+The check passed.")}),
        )]),
        calls(vec![call(
            "c5",
            "answer",
            json!({"summary": "Wrote the note"}),
        )]),
        says("A second answer nobody asked for."),
    ];
    let messages = run(&root, script).await?;
    let results: Vec<(String, bool, String)> = (messages.iter())
        .filter_map(|message| match message {
            AgentMessage::ToolResult {
                tool_name,
                is_error,
                content,
                ..
            } => Some((
                tool_name.clone(),
                *is_error,
                yi_types::message::join_text(content, ""),
            )),
            _ => None,
        })
        .collect();
    let refused = results.get(1).ok_or("no answer result")?;
    assert!(refused.1 && refused.2.contains("held"), "{results:?}");
    let edited = results.get(3).ok_or("no edit result")?;
    assert!(!edited.1, "the held draft takes an edit: {results:?}");
    assert_eq!(
        answers(&messages),
        vec!["Wrote the note.\nThe check passed.".to_owned()],
        "{results:?}"
    );
    let last = messages.last().ok_or("no messages")?;
    assert!(
        matches!(last, AgentMessage::Assistant { usage, .. } if usage.unknown),
        "an appended answer reports no usage, so the next context estimate is not read as empty"
    );
    Ok(())
}

/// Dies with the stop ladder re-driving every text stop: a reply with open work is sent back
/// once, and the reply after that ends the turn.
#[tokio::test]
async fn a_text_stop_with_open_work_is_sent_back_once() -> TestResult {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        says("Done."),
        says("Really done."),
        says("A third reply nobody asked for."),
    ];
    let messages = run(&Scratch::new("yi-answer-text-stop")?, script).await?;
    assert_eq!(notes(&messages, INTERCEPT_CUSTOM_TYPE), 1);
    assert_eq!(
        answers(&messages),
        vec!["Done.".to_owned(), "Really done.".to_owned()]
    );
    Ok(())
}

/// Dies with a host note queued on the answer's own turn reopening it: three writes with no
/// list earn a nudge in the batch that answers, and the answer must still be the last word.
#[tokio::test]
async fn a_host_note_queued_on_the_answer_turn_is_dropped() -> TestResult {
    let write = |id: &str, path: &str| call(id, "write", json!({"path": path, "content": "x\n"}));
    let script = vec![
        calls(vec![
            write("c1", "a.txt"),
            write("c2", "b.txt"),
            write("c3", "c.txt"),
            call(
                "c4",
                "answer",
                json!({"summary": "Wrote three files", "text": "Wrote three files."}),
            ),
        ]),
        says("A second answer nobody asked for."),
    ];
    assert!(
        yi_runtime::levers::get().todo_first_list_work <= 3,
        "three writes earn the nudge"
    );
    let messages = run(&Scratch::new("yi-answer-quiet")?, script).await?;
    assert_eq!(answers(&messages), vec!["Wrote three files.".to_owned()]);
    assert_eq!(notes(&messages, "todo_nudge"), 0);
    Ok(())
}

/// Dies with a sent-back reply printed twice, or a turn that runs on after it: a reply on
/// screen that passes once the work is done stands, even beside another call in its batch.
#[tokio::test]
async fn a_sent_back_reply_that_passes_stands_without_a_second_copy() -> TestResult {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        says("Done."),
        calls(vec![
            call(
                "c2",
                "todo",
                json!({"op": "done", "label": "write the note", "evidence": "`true` exit 0"}),
            ),
            call("c3", "answer", json!({"summary": "Wrote the note"})),
        ]),
        says("A second answer nobody asked for."),
    ];
    let messages = run(&Scratch::new("yi-answer-stands")?, script).await?;
    assert_eq!(answers(&messages), vec!["Done.".to_owned()]);
    Ok(())
}

/// Dies with the last word refusing the answer: under the turn cap the `answer` call is still
/// the answer, so the run neither ends empty nor asks again.
#[tokio::test]
async fn an_answer_on_the_last_word_is_still_the_answer() -> TestResult {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        calls(vec![call(
            "c2",
            "answer",
            json!({"summary": "Out of turns", "text": "Out of turns; the note is unwritten."}),
        )]),
        says("A second answer nobody asked for."),
    ];
    let messages = run_capped(&Scratch::new("yi-answer-capped")?, script, Some(2), None).await?;
    assert_eq!(
        answers(&messages),
        vec!["Out of turns; the note is unwritten.".to_owned()]
    );
    Ok(())
}

/// Dies with an answered turn swallowing what the user typed while it ran: input queued
/// behind the answer still opens the next turn, which answers it.
#[tokio::test]
async fn input_typed_during_the_turn_opens_the_next_one() -> TestResult {
    let answer = |id: &str, text: &str| {
        calls(vec![call(
            id,
            "answer",
            json!({"summary": text, "text": text}),
        )])
    };
    let script = vec![answer("c1", "First."), answer("c2", "Second.")];
    let root = Scratch::new("yi-answer-typed")?;
    let messages = run_capped(&root, script, None, Some("and the next")).await?;
    assert_eq!(
        answers(&messages),
        vec!["First.".to_owned(), "Second.".to_owned()]
    );
    Ok(())
}
