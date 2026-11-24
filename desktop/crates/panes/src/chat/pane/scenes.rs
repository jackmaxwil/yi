//! The pane's scenes: the mockup's conversation and two real recordings, stepped into the model.

use yd_core::ids::Millis;
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::reduce::turns::CallId;
use yd_core::wire::replay::{play, play_line};
use yd_ui::win::Hub;

use yd_ui::pane::SCENE_PANE;

use super::Chat;

/// The mockup's thread, by its address slug (record 3.9).
const THREAD: &str = "rate-limit-uploads";
/// 17:57:58 UTC on the mockup's day: the user's prompt of turn 2 (record 3.8).
const ASKED: u64 = 1_794_679_078_000;
/// Turn 1 began at 17:40:12, 1,066 seconds before turn 2.
const TURN_ONE: u64 = ASKED - 1_066_000;

const PROMPT: &str = include_str!("../../../../../fixtures/mockup/data/UM.json");
const ANSWER: &str = include_str!("../../../../../fixtures/mockup/data/A1.json");
const WRITING: &str = include_str!("../../../../../fixtures/mockup/data/A2.json");
const TURN: &str = include_str!("../../../../../fixtures/wire/turn.jsonl");
const ALLOW: &str = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
const TOOLS: &str = include_str!("../../../../../fixtures/wire/tools-turn.jsonl");
/// A real server with no thread: it is up and lists none.
const NONE: &str = include_str!("../../../../../fixtures/wire/empty-list.jsonl");

/// Yi is up and has no thread, so the pane has none to show.
pub fn empty(hub: &mut Hub) {
    let _ = play(&mut hub.model, NONE);
}

/// A JSON string literal of `text`.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A page string as Markdown: its inline code marks become backticks and the caret goes.
fn markdown(page: &str) -> String {
    page.trim()
        .replace("<code>", "`")
        .replace("</code>", "`")
        .replace("<span class=\\\"caret\\\"></span>", "")
}

fn recv(frame: &str) -> String {
    format!(r#"{{"dir":"recv","frame":{frame}}}"#)
}

fn update(body: &str) -> String {
    recv(&format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{THREAD}","update":{{{body}}}}}}}"#
    ))
}

/// The record's model, with its usage and cost: output tokens and dollars.
fn assistant(content: &str, output: u64, cost: &str) -> String {
    format!(
        r#"{{"role":"assistant","content":[{content}],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","usage":{{"input":0,"output":{output},"cacheRead":0,"cacheWrite":0,"totalTokens":{output},"cost":{{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":{cost}}}}},"stopReason":"stop","timestamp":0}}"#
    )
}

fn user(text: &str) -> String {
    format!(r#"{{"role":"user","content":{text},"timestamp":0,"attribution":"user"}}"#)
}

fn call(id: &str, name: &str, args: &str) -> String {
    format!(r#"{{"type":"toolCall","id":"{id}","name":"{name}","arguments":{args}}}"#)
}

/// Steps lines into the model, each at its own time.
struct Script<'a> {
    model: &'a mut Model,
    seq: u64,
}

impl Script<'_> {
    fn at(&mut self, at: u64) -> &mut Self {
        step(self.model, Msg::Tick(Millis(at)));
        self
    }

    fn line(&mut self, line: &str) -> &mut Self {
        // A scene line that does not decode is a scene bug the tests catch by what is missing.
        let _ = play_line(self.model, line);
        self
    }

    fn event(&mut self, event: &str) -> &mut Self {
        self.child_event("", event)
    }

    fn child_event(&mut self, child: &str, event: &str) -> &mut Self {
        let seq = self.seq;
        self.seq = seq.saturating_add(1);
        let child = if child.is_empty() {
            String::new()
        } else {
            format!(r#","childId":"{child}""#)
        };
        self.line(&update(&format!(
            r#""sessionUpdate":"_yi/event","seq":{seq}{child},"event":{event}"#
        )))
    }

    fn tool(&mut self, at: u64, call: [&str; 3], output: &str, details: &str) -> &mut Self {
        let [id, name, args] = call;
        let end = format!(
            r#"{{"type":"tool_execution_end","toolCallId":"{id}","toolName":"{name}","result":{{"content":[{{"type":"text","text":{}}}],"details":{details}}},"isError":false}}"#,
            quote(output)
        );
        self.at(at)
            .event(&format!(
                r#"{{"type":"tool_execution_start","toolCallId":"{id}","toolName":"{name}","args":{args}}}"#
            ))
            .at(at.saturating_add(50))
            .event(&end)
    }
}

/// The mockup's conversation (record 3.8) as the server sends it: turn 2's steps and answer,
/// a child's ask with a second queued behind it, and the next answer streaming.
pub(crate) fn mockup(hub: &mut Hub) {
    let mut s = Script {
        model: &mut hub.model,
        seq: 0,
    };
    let options = r#"[{"configId":"model","name":"Model","type":"select","currentValue":"anthropic/claude-opus-5-5","options":[{"value":"anthropic/claude-opus-5-5","name":"Opus 5.5"}]},{"configId":"thought_level","name":"Thinking level","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]},{"configId":"mode","name":"Permission mode","type":"select","currentValue":"ask","options":[{"value":"ask","name":"ask"}]}]"#;
    s.at(TURN_ONE)
        .line(&recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ))
        .line(r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}}"#)
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}","configOptions":{options}}}}}"#
        )))
        .line(&update(r#""sessionUpdate":"_yi/name","name":"Rate-limit the upload endpoint""#))
        .line(&update(r#""sessionUpdate":"_yi/workdir","cwd":"~/atlas-api","lane":null"#));
    let title = quote("Rate-limit the upload endpoint");
    let found = format!(
        r#"{{"type":"text","text":{}}}"#,
        quote("Found the upload route.")
    );
    s.event(r#"{"type":"agent_start"}"#)
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            user(&title)
        ))
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            user(&title)
        ))
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            assistant("", 0, "0")
        ))
        .at(TURN_ONE + 9_000)
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            assistant(&found, 0, "0")
        ));
    // Three files were in context (record 3.8); the server stores them as addresses after
    // the words typed. The record names no files, so these are the turn's own.
    let typed = PROMPT.trim().strip_suffix('"').unwrap_or(PROMPT);
    let prompt = &format!(
        "{typed} yi://file/src/routes/upload.rs yi://file/src/middleware/mod.rs yi://file/src/main.rs\""
    );
    const READ: &str = r#"{"path":"src/routes/upload.rs"}"#;
    const GREP: &str = r#"{"pattern":"RateLimit"}"#;
    const NEW: &str = r#"{"path":"src/middleware/rate_limit.rs"}"#;
    const EDIT: &str = r#"{"path":"src/routes/upload.rs"}"#;
    let calls = [
        call("c1", "read", READ),
        call("c2", "grep", GREP),
        call("c3", "edit", NEW),
        call("c4", "edit", EDIT),
    ]
    .join(",");
    // The record gives only a read's line count and a search's match count, so each line is
    // its own number.
    let lines: Vec<String> = (1..=84).map(|n| n.to_string()).collect();
    s.at(ASKED)
        .line(&update(
            r#""sessionUpdate":"state_update","state":"running""#,
        ))
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            user(prompt)
        ))
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            user(prompt)
        ))
        .at(ASKED + 100)
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            assistant("", 0, "0")
        ))
        .at(ASKED + 200)
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            assistant(&calls, 0, "0")
        ))
        .tool(ASKED + 1_000, ["c1", "read", READ], &lines.join("\n"), "{}")
        .tool(ASKED + 2_000, ["c2", "grep", GREP], "1\n2\n3", "{}")
        .tool(
            ASKED + 3_000,
            ["c3", "edit", NEW],
            "",
            r#"{"added":48,"removed":0}"#,
        )
        .tool(
            ASKED + 3_100,
            ["c4", "edit", EDIT],
            "",
            r#"{"added":6,"removed":1}"#,
        );
    let answer = format!(r#"{{"type":"text","text":{}}}"#, markdown(ANSWER));
    s.at(ASKED + 3_200)
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            assistant("", 0, "0")
        ))
        .at(ASKED + 3_400)
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            assistant(&answer, 612, "0.38")
        ))
        .line(&update(
            r#""sessionUpdate":"usage_update","used":41200,"size":200000"#,
        ));
    let child = r#""sessionUpdate":"_yi/subagent_update","id":"tests-upload","name":"Tests · upload","status":"running","activity":"waiting","toolUseCount":0,"tokenCount":0,"task":"to check the 429 answer""#;
    let test = call("c5", "bash", r#"{"command":"cargo test -p api upload::"}"#);
    let ask = |id: &str| {
        recv(&format!(
            r#"{{"jsonrpc":"2.0","id":"{id}","method":"session/request_permission","params":{{"sessionId":"{THREAD}","title":"bash requires permission","description":"sandbox on · no network · writes nothing · about 20 s","options":[{{"optionId":"allow_once","name":"Allow once","kind":"allow_once"}},{{"optionId":"allow_always","name":"Always allow","kind":"allow_always"}},{{"optionId":"reject_once","name":"Reject","kind":"reject_once"}}]}}}}"#
        ))
    };
    s.at(ASKED + 4_000)
        .line(&update(child))
        .child_event(
            "tests-upload",
            &format!(r#"{{"type":"message_end","message":{}}}"#, assistant(&test, 0, "0")),
        )
        .line(&ask("perm-1"))
        .event(r#"{"type":"permission_requested","toolCallId":"c5","title":"bash requires permission","description":"sandbox on · no network · writes nothing · about 20 s"}"#)
        .line(&update(r#""sessionUpdate":"state_update","state":"requires_action""#))
        .at(ASKED + 5_000)
        .line(&ask("perm-2"));
    let writing = format!(
        r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":{}}}}}"#,
        markdown(WRITING)
    );
    s.at(ASKED + 44_000)
        .event(&format!(r#"{{"type":"message_start","message":{}}}"#, assistant("", 76, "0")))
        .event(r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#)
        .event(&writing)
        .at(ASKED + 46_000);
}

/// The mockup's state with the second ask of the queue shown.
pub(crate) fn queue(hub: &mut Hub) {
    mockup(hub);
    if let Some(chat) = hub.panes.get_mut::<Chat>(Some(SCENE_PANE)) {
        chat.pick = 1;
    }
}

/// The real turn recording, stopped while the answer streams.
pub(crate) fn streaming(hub: &mut Hub) {
    for line in TURN.lines() {
        let _ = play_line(&mut hub.model, line);
        if line.contains("\"text_delta\"") {
            break;
        }
    }
}

/// The real approval recording, played whole: the ask was allowed and the command ran.
pub(crate) fn allowed(hub: &mut Hub) {
    let _ = play(&mut hub.model, ALLOW);
}

/// The recorded turn of bash, read, grep, ipython and edit calls, with a command, a read, a
/// failed cell and the edit open.
pub(crate) fn tools(hub: &mut Hub) {
    let _ = play(&mut hub.model, TOOLS);
    if let Some(chat) = hub.panes.get_mut::<Chat>(Some(SCENE_PANE)) {
        chat.steps = ["t1", "t2", "t5", "t7"].map(CallId::new).to_vec();
    }
}

/// A thread of `turns` turns, each a prompt and its answer, as the server streams them.
#[cfg(test)]
pub(crate) fn long(hub: &mut Hub, turns: u64) {
    let mut s = Script {
        model: &mut hub.model,
        seq: 0,
    };
    s.at(TURN_ONE)
        .line(&recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ))
        .line(r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}}"#)
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}","configOptions":[]}}}}"#
        )))
        .line(&update(r#""sessionUpdate":"_yi/workdir","cwd":"~/atlas-api","lane":null"#));
    for turn in 0..turns {
        let asked = user(&quote(&format!("Question {turn}")));
        let said = format!(
            r#"{{"type":"text","text":{}}}"#,
            quote(&format!("Answer {turn}"))
        );
        s.at(TURN_ONE.saturating_add(turn.saturating_mul(1_000)))
            .event(r#"{"type":"agent_start"}"#)
            .event(&format!(r#"{{"type":"message_start","message":{asked}}}"#))
            .event(&format!(r#"{{"type":"message_end","message":{asked}}}"#))
            .event(&format!(
                r#"{{"type":"message_start","message":{}}}"#,
                assistant("", 0, "0")
            ))
            .event(&format!(
                r#"{{"type":"message_end","message":{}}}"#,
                assistant(&said, 1, "0")
            ))
            .event(r#"{"type":"agent_end","messages":[]}"#);
    }
}

/// The compound command [`dogfood`]'s second call asks to run.
#[cfg(test)]
pub(crate) const LONG: &str = "git status --short && git add -A && git commit -m \"Add the --by flag with a test\" && cargo test 2>&1 | tail -20";

/// The relay's model as the dogfood server named it, its effort and its mode.
#[cfg(test)]
const DOGFOOD_OPTIONS: &str = r#"[{"configId":"model","name":"Model","type":"select","currentValue":"bedrock/claude-sonnet-5-5","options":[{"value":"anthropic/claude-opus-5-5","name":"Anthropic · Claude Opus 5.5"},{"value":"bedrock/claude-sonnet-5-5","name":"Bedrock · Claude Sonnet 5.5"}]},{"configId":"thought_level","name":"Thinking level","type":"select","currentValue":"high","options":[{"value":"low","name":"low"},{"value":"high","name":"high"}]},{"configId":"mode","name":"Permission mode","type":"select","currentValue":"ask","options":[{"value":"ask","name":"ask"}]}]"#;

/// The dogfood round's thread: a finished turn, then a turn whose edit waits on its ask,
/// with the edit's diff as Yi sends it, and a compound command behind it.
#[cfg(test)]
pub(crate) fn dogfood(hub: &mut Hub) {
    edit_asked(hub, UNDER_SRC);
}

/// The answers Yi's grants give an edit under `src/`, and one at the tree's root.
#[cfg(test)]
const UNDER_SRC: &str = r#"[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"},{"optionId":"allow_always","name":"Always allow edits under src","kind":"allow_always"},{"optionId":"allow_always_1","name":"Always allow edits anywhere in this tree","kind":"allow_always"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]"#;
#[cfg(test)]
const AT_ROOT: &str = r#"[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"},{"optionId":"allow_always","name":"Always allow this exact call","kind":"allow_always"},{"optionId":"allow_always_1","name":"Always allow edits anywhere in this tree","kind":"allow_always"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]"#;

/// [`dogfood`] with the answers Yi offers an edit at the tree's root.
#[cfg(test)]
pub(crate) fn at_root(hub: &mut Hub) {
    edit_asked(hub, AT_ROOT);
}

#[cfg(test)]
fn edit_asked(hub: &mut Hub, options: &str) {
    let mut s = Script {
        model: &mut hub.model,
        seq: 0,
    };
    s.at(TURN_ONE)
        .line(&recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ))
        .line(r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/w/tally","mcpServers":[]}}}"#)
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}","configOptions":{DOGFOOD_OPTIONS}}}}}"#
        )))
        .line(&update(r#""sessionUpdate":"_yi/workdir","cwd":"/w/tally","lane":null"#));
    let asked = user(&quote("What does this project do?"));
    let said = format!(
        r#"{{"type":"text","text":{}}}"#,
        quote("Tally sums CSV text.")
    );
    s.event(r#"{"type":"agent_start"}"#)
        .event(&format!(r#"{{"type":"message_start","message":{asked}}}"#))
        .event(&format!(r#"{{"type":"message_end","message":{asked}}}"#))
        .at(TURN_ONE + 1_000)
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            assistant("", 0, "0")
        ))
        .at(TURN_ONE + 26_000)
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            assistant(&said, 0, "0")
        ))
        .event(r#"{"type":"agent_end","messages":[]}"#);
    let edit = r#"{"patch":"[src/main.rs#1D52]\nPUT 3.=3:\n+fn main() {\n+}"}"#;
    let bash = format!(r#"{{"command":{}}}"#, quote(LONG));
    let calls = [call("e1", "edit", edit), call("b1", "bash", &bash)].join(",");
    let asked = user(&quote("Add the --by flag from the README, with a test."));
    s.at(ASKED)
        .line(&update(
            r#""sessionUpdate":"state_update","state":"running""#,
        ))
        .event(&format!(r#"{{"type":"message_start","message":{asked}}}"#))
        .event(&format!(r#"{{"type":"message_end","message":{asked}}}"#))
        .event(&format!(
            r#"{{"type":"message_start","message":{}}}"#,
            assistant("", 0, "0")
        ))
        .event(&format!(
            r#"{{"type":"message_end","message":{}}}"#,
            assistant(&calls, 0, "0")
        ))
        .event(&format!(
            r#"{{"type":"tool_execution_start","toolCallId":"e1","toolName":"edit","args":{edit}}}"#
        ));
    // The diff Yi's permission bridge sends: a git patch naming the absolute path.
    let patch = quote(
        "--- a//w/home/.yi/lanes/18353d274aaa73f1/0/src/main.rs\n+++ b//w/home/.yi/lanes/18353d274aaa73f1/0/src/main.rs\n@@ -1,2 +1,3 @@\n use std::io::Read;\n-fn main() {}\n+fn main() {\n+}\n",
    );
    let subject = format!(
        r#"{{"type":"tool_call","toolCall":{{"toolCallId":"e1","content":[{{"type":"diff","changes":[{{"operation":"modify","path":"/w/home/.yi/lanes/18353d274aaa73f1/0/src/main.rs"}}],"patch":{{"format":"git_patch","text":{patch}}}}}]}}}}"#
    );
    s.at(ASKED + 2_000)
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":"perm-e1","method":"session/request_permission","params":{{"sessionId":"{THREAD}","title":"edit requires permission","description":"tool invocation is not read-only: edit","options":{options},"subject":{subject}}}}}"#
        )))
        .event(r#"{"type":"permission_requested","toolCallId":"e1","title":"edit requires permission","description":"tool invocation is not read-only: edit"}"#)
        .line(&update(r#""sessionUpdate":"state_update","state":"requires_action""#))
        .at(ASKED + 9_000);
}

/// [`dogfood`] with its edit allowed: the compound command waits on its ask.
#[cfg(test)]
pub(crate) fn commanding(hub: &mut Hub) {
    dogfood(hub);
    let mut s = Script {
        model: &mut hub.model,
        seq: 12,
    };
    let bash = format!(r#"{{"command":{}}}"#, quote(LONG));
    let why = quote(&format!("tool invocation is not read-only: {LONG}"));
    let options = r#"[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"},{"optionId":"allow_always","name":"Always allow","kind":"allow_always"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]"#;
    s.at(ASKED + 10_000)
        .event(r#"{"type":"permission_resolved","toolCallId":"e1","allowed":true}"#)
        .line(&update(r#""sessionUpdate":"state_update","state":"running""#))
        .tool(ASKED + 11_000, ["e1", "edit", "{}"], "updated", r#"{"added":2,"removed":1}"#)
        .at(ASKED + 12_000)
        .event(&format!(
            r#"{{"type":"tool_execution_start","toolCallId":"b1","toolName":"bash","args":{bash}}}"#
        ))
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":"perm-b1","method":"session/request_permission","params":{{"sessionId":"{THREAD}","title":"bash requires permission","description":{why},"options":{options}}}}}"#
        )))
        .event(&format!(
            r#"{{"type":"permission_requested","toolCallId":"b1","title":"bash requires permission","description":{why}}}"#
        ))
        .line(&update(r#""sessionUpdate":"state_update","state":"requires_action""#))
        .at(ASKED + 20_000);
}

/// [`dogfood`] with a prompt whose one word is longer than the pane is wide.
#[cfg(test)]
pub(crate) fn long_word(hub: &mut Hub) {
    dogfood(hub);
    let mut s = Script {
        model: &mut hub.model,
        seq: 12,
    };
    let asked = user(&quote(
        "See `src/very/long/module/path/with_a_really_long_name_that_keeps_going.rs` now",
    ));
    s.at(ASKED + 10_000)
        .event(&format!(r#"{{"type":"message_start","message":{asked}}}"#))
        .event(&format!(r#"{{"type":"message_end","message":{asked}}}"#));
}

/// [`commanding`] stopped while its ask waits: the command ends as Yi ends one a stop caught.
#[cfg(test)]
pub(crate) fn stopped(hub: &mut Hub) {
    commanding(hub);
    let mut s = Script {
        model: &mut hub.model,
        seq: 17,
    };
    let aborted = r#"{"role":"assistant","content":[],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"aborted","timestamp":0}"#;
    s.at(ASKED + 45_500)
        .event(r#"{"type":"tool_execution_end","toolCallId":"b1","toolName":"bash","result":{"content":[{"type":"text","text":"[command aborted]\nexit code: -1"}],"details":{}},"isError":true}"#)
        .event(&format!(r#"{{"type":"message_start","message":{aborted}}}"#))
        .event(&format!(r#"{{"type":"message_end","message":{aborted}}}"#))
        .event(r#"{"type":"agent_end","messages":[]}"#)
        .line(&idle());
}

/// The line Yi ends a stopped turn with: the thread is idle.
#[cfg(test)]
pub(crate) fn idle() -> String {
    update(r#""sessionUpdate":"state_update","state":"idle","stopReason":"cancelled""#)
}
