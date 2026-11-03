use crate::scratch::Scratch;
use crate::todo_mirror::{memory_store, session, wired};

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::ProviderStream;
use yi_runtime::todo::coupling::{DONE_BLOCKED_TEXT, INTERCEPT_CUSTOM_TYPE, gate::LADDER_TOP};
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

/// Dies with a sent-back reply printed twice, cut to a status line for its parent, or a turn
/// that runs on after it: a reply on screen that passes once the work is done stands as written.
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

/// Dies with the last word refusing the answer or dropping its status line: under the turn cap
/// the `answer` call is still the answer, so the run neither ends empty nor asks again.
#[tokio::test]
async fn an_answer_on_the_last_word_is_still_the_answer() -> TestResult {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        answered("Problem: Out of turns.", "The note is unwritten."),
        says("A second answer nobody asked for."),
    ];
    let messages = run_capped(&Scratch::new("yi-answer-capped")?, script, Some(2), None).await?;
    let reply = "Problem: Out of turns.\n\nThe note is unwritten.".to_owned();
    assert_eq!(answers(&messages), vec![reply]);
    Ok(())
}

/// Dies with a draft held over to the last word losing its status line: the summary that
/// resubmits it under the turn cap heads the draft the earlier refusal kept.
#[tokio::test]
async fn a_draft_resubmitted_on_the_last_word_keeps_its_status_line() -> TestResult {
    let script = vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        answered("Problem: The note is open.", "The note is unwritten."),
        summary_only("Problem: Out of turns."),
        says("A second answer nobody asked for."),
    ];
    let root = Scratch::new("yi-answer-capped-held")?;
    let messages = run_capped(&root, script, Some(3), None).await?;
    let reply = "Problem: Out of turns.\n\nThe note is unwritten.".to_owned();
    assert_eq!(answers(&messages), vec![reply]);
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

fn answered(summary: &str, text: &str) -> AgentMessage {
    calls(vec![call(
        "c1",
        "answer",
        json!({"summary": summary, "text": text}),
    )])
}

/// Dies with the summary dropped before anyone reads it: the status line the user scans first
/// must head the reply, above the detail.
#[tokio::test]
async fn the_summary_heads_the_reply_the_user_reads() -> TestResult {
    let text = "The parser read every date as UTC.\n\n- It reads the zone from the file now.\n- The full check passes.";
    let script = vec![answered("Done: Fixed the date bug.", text)];
    let messages = run(&Scratch::new("yi-answer-headed")?, script).await?;
    assert_eq!(
        answers(&messages),
        vec![format!("Done: Fixed the date bug.\n\n{text}")]
    );
    Ok(())
}

/// Dies with a status line glued onto data: a structured child's JSON answer must reach the
/// parent bare, or the parse picks up a bracket from the summary instead of the value.
#[tokio::test]
async fn a_json_answer_reaches_its_reader_bare() -> TestResult {
    let data = r#"{"files": ["src/a.rs", "src/b.rs"]}"#;
    let script = vec![answered("Done: Read [both] files.", data)];
    let messages = run(&Scratch::new("yi-answer-json")?, script).await?;
    assert_eq!(answers(&messages), vec![data.to_owned()]);
    Ok(())
}

/// Dies with a bracket in the summary parsed as the data: prose around inline JSON is what a
/// `--schema` reader falls back to, so the line must not put a bracket ahead of the value.
#[tokio::test]
async fn a_bracket_in_the_summary_never_becomes_the_parsed_answer() -> TestResult {
    let data = r#"The review found two issues: {"issues": 2}"#;
    let script = vec![answered("Done: Found [2] issues.", data)];
    let replies = answers(&run(&Scratch::new("yi-answer-inline-json")?, script).await?);
    let parsed = yi_runtime::schema::extract(replies.first().map_or("", String::as_str))?;
    assert_eq!(parsed, json!({"issues": 2}));
    Ok(())
}

fn refused_done(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::ToolResult { tool_name, is_error: true, content, .. }
        if tool_name == "answer"
            && yi_types::message::join_text(content, "").contains(DONE_BLOCKED_TEXT))
}

/// Dies with Done passing over an item that waits on the user: that reply is sent back, and
/// the status that names the wait heads what the user reads.
#[tokio::test]
async fn done_is_refused_while_an_item_is_blocked() -> TestResult {
    let mut script = blocked_note();
    script.extend([
        answered("Done: Wrote the note.", "The note waits for its title."),
        summary_only("Ready: Pick the note's title."),
        says("A second answer nobody asked for."),
    ]);
    let messages = run(&Scratch::new("yi-answer-done-blocked")?, script).await?;
    let refused = messages.iter().any(refused_done);
    assert!(refused, "the Done reply was not sent back");
    assert_eq!(
        answers(&messages),
        vec!["Ready: Pick the note's title.\n\nThe note waits for its title.".to_owned()]
    );
    Ok(())
}

fn blocked_note() -> Vec<AgentMessage> {
    let block =
        json!({"op": "block", "label": "write the note", "on": "user", "note": "which title?"});
    vec![
        calls(vec![call(
            "c1",
            "todo",
            json!({"op": "init", "items": ["write the note"]}),
        )]),
        calls(vec![call("c2", "todo", block)]),
    ]
}

fn summary_only(summary: &str) -> AgentMessage {
    calls(vec![call("c4", "answer", json!({"summary": summary}))])
}

/// Dies with a refused plain Done kept as the answer: a summary alone cannot keep a reply on
/// screen that claims Done over an item waiting on the user, so the fix comes as new text.
#[tokio::test]
async fn a_plain_done_over_a_blocked_item_cannot_stand_on_a_summary() -> TestResult {
    let ready = "Ready: Pick the note's title.";
    let mut script = blocked_note();
    script.extend([
        says("Done."),
        summary_only(ready),
        answered(ready, "The note waits for its title."),
        says("A second answer nobody asked for."),
    ]);
    let messages = run(&Scratch::new("yi-answer-plain-done")?, script).await?;
    assert_eq!(notes(&messages, INTERCEPT_CUSTOM_TYPE), 1);
    let reply = format!("{ready}\n\nThe note waits for its title.");
    assert_eq!(answers(&messages), vec!["Done.".to_owned(), reply]);
    Ok(())
}

/// Dies with a repeated Done let through on its second try, or held for good: the claim is
/// sent back while the list stands still, and let go once the send-back ladder tops out.
#[tokio::test]
async fn a_repeated_done_over_a_blocked_item_is_let_go_at_the_ladder_top() -> TestResult {
    let done = || answered("Done: Wrote the note.", "The note waits for its title.");
    let mut script = blocked_note();
    script.extend((0..=LADDER_TOP).map(|_| done()));
    script.push(says("A second answer nobody asked for."));
    let messages = run(&Scratch::new("yi-answer-done-ladder")?, script).await?;
    let refusals = messages
        .iter()
        .filter(|message| refused_done(message))
        .count();
    assert_eq!(refusals, usize::from(LADDER_TOP));
    let reply = "Done: Wrote the note.\n\nThe note waits for its title.".to_owned();
    assert_eq!(answers(&messages), vec![reply]);
    Ok(())
}

/// Dies with data read as a status: a blocked child's JSON whose first key is `done` is no
/// Done line, so it passes as written.
#[tokio::test]
async fn a_json_answer_with_a_done_key_is_not_a_status_line() -> TestResult {
    let data = r#"{"done": false, "waits_on": "the note's title"}"#;
    let mut script = blocked_note();
    script.extend([
        answered("Ready: Pick the note's title.", data),
        says("A second answer."),
    ]);
    let messages = run(&Scratch::new("yi-answer-done-key")?, script).await?;
    assert_eq!(answers(&messages), vec![data.to_owned()]);
    Ok(())
}

/// Dies with a one-line reply refused for having no `text`: the summary alone is the reply.
#[tokio::test]
async fn a_summary_alone_is_the_whole_reply() -> TestResult {
    let script = vec![
        summary_only("Done: Said hello back."),
        says("A second answer."),
    ];
    let messages = run(&Scratch::new("yi-answer-summary-only")?, script).await?;
    assert_eq!(
        answers(&messages),
        vec!["Done: Said hello back.".to_owned()]
    );
    Ok(())
}

/// Dies with the status line dropped from a reply that opens on a link: only text that
/// parses whole as JSON is data, and Markdown is not.
#[tokio::test]
async fn a_reply_that_opens_on_a_link_keeps_its_status_line() -> TestResult {
    let text = "[The pull request](https://git.example.test/pulls/voice) is green.";
    let script = vec![answered("Ready: Merge the pull request.", text)];
    let messages = run(&Scratch::new("yi-answer-link")?, script).await?;
    let reply = format!("Ready: Merge the pull request.\n\n{text}");
    assert_eq!(answers(&messages), vec![reply]);
    Ok(())
}

/// Dies with a status line glued onto a scalar: `42` under a summary holds no JSON value, so
/// an integer `--schema` answer exits 3.
#[tokio::test]
async fn a_scalar_answer_reaches_its_reader_bare() -> TestResult {
    let script = vec![answered("Done: Counted the files.", "42")];
    let messages = run(&Scratch::new("yi-answer-scalar")?, script).await?;
    assert_eq!(answers(&messages), vec!["42".to_owned()]);
    Ok(())
}

/// Dies with a status line inventing data: a prose reply holds no JSON, so a reader looking
/// for some must not find the summary's `[2]`.
#[tokio::test]
async fn a_status_line_never_invents_json_in_a_prose_reply() -> TestResult {
    let script = vec![answered(
        "Problem: [2] checks fail.",
        "Two checks fail on the parser.",
    )];
    let replies = answers(&run(&Scratch::new("yi-answer-no-json")?, script).await?);
    let reply = replies.first().map_or("", String::as_str);
    assert!(yi_runtime::schema::extract(reply).is_err(), "{reply}");
    Ok(())
}
/// Dies with a persisted turn losing its terminate decision on resume: the entry the store
/// holds for a turn-ending `answer` call carries the flag, so a resumed session reads the turn as ended.
#[tokio::test]
async fn a_turn_ending_answer_persists_its_terminate_flag() -> TestResult {
    let root = Scratch::new("yi-answer-persist-terminate")?;
    let store = memory_store();
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
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
    ]);
    let mut session = session(Arc::clone(&provider));
    session.attach_store(Arc::clone(&store))?;
    wired(&mut session, &root, provider);
    session.prompt("write the note")?;
    session.wait_idle().await;
    let stored =
        yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery::default())?;
    let flag = (stored.iter()).find_map(|entry| match entry {
        yi_types::entry::Entry::Message {
            message: AgentMessage::ToolResult { tool_call_id, .. },
            terminate,
            ..
        } if tool_call_id == "c3" => Some(*terminate),
        _ => None,
    });
    assert_eq!(
        flag,
        Some(Some(true)),
        "the persisted answer entry must carry terminate: Some(true), got {flag:?} in {} entries",
        stored.len()
    );
    Ok(())
}
