//! The document's scenes: the mockup's thread (record 3.8) as the server sends it.

use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::reduce::turns::ToolCall;
use yd_core::wire::replay::play_line;
use yd_ui::pane::SCENE_PANE;
use yd_ui::win::Hub;

use super::{Doc, Pose};
use crate::chat_doc::hover::Block;

/// The mockup's thread, by its address slug (record 3.9).
pub(crate) const THREAD: &str = "rate-limit-uploads";
pub(crate) const FORK_ID: &str = "fork-sliding-window";
/// 17:57:58 UTC on the mockup's day; turn 2's prompt lands 0.6 s later.
const ASKED: u64 = 1_794_679_078_000;
/// Turn 1 began at 17:40:12.
const TURN_ONE: u64 = ASKED - 1_066_000;
/// Turn 2 began here and worked 14 seconds before the child's ask held it.
const TURN_TWO: u64 = ASKED + 600;
const HELD: u64 = TURN_TWO + 14_000;
/// The clock of the picture: the ask has waited 0:42.
pub(crate) const NOW: u64 = HELD + 42_000;

const PROMPT: &str = include_str!("../../../../../fixtures/mockup/data/UM.json");
const ANSWER: &str = include_str!("../../../../../fixtures/mockup/data/A1.json");
const WRITING: &str = include_str!("../../../../../fixtures/mockup/data/A2.json");
const NEW_FILE: &str = include_str!("../../../../../fixtures/mockup/data/R.json");
const HUNK: &str = include_str!("../../../../../fixtures/mockup/data/U.json");

/// A JSON string literal of `text`.
pub(crate) fn quote(text: &str) -> String {
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
        .trim_matches('"')
        .replace("<code>", "`")
        .replace("</code>", "`")
        .replace("<span class=\\\"caret\\\"></span>", "")
}

/// The page's diff rows (`[sign, line, text]`) whose line `keep` takes, as unified diff lines.
// Incident: this crate cannot name serde_json, so the rows are read through a call's arguments.
fn rows(page: &str, keep: impl Fn(u64) -> bool) -> Vec<String> {
    let holder = ToolCall {
        args: page.parse().unwrap_or_default(),
        ..ToolCall::default()
    };
    let all = holder.args.as_array().cloned().unwrap_or_default();
    all.iter()
        .filter_map(|row| {
            let sign = row.get(0)?.as_str()?;
            let line = row.get(1)?.as_u64()?;
            let text = row.get(2)?.as_str()?;
            keep(line).then(|| format!("{sign}{text}"))
        })
        .collect()
}

pub(crate) fn recv(frame: &str) -> String {
    format!(r#"{{"dir":"recv","frame":{frame}}}"#)
}

pub(crate) fn send(frame: &str) -> String {
    format!(r#"{{"dir":"send","frame":{frame}}}"#)
}

pub(crate) fn update(thread: &str, body: &str) -> String {
    recv(&format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{thread}","update":{{{body}}}}}}}"#
    ))
}

/// An answer of the record's model: its content, output tokens and dollars.
pub(crate) fn assistant(content: &str, output: u64, cost: &str) -> String {
    format!(
        r#"{{"role":"assistant","content":[{content}],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","usage":{{"input":0,"output":{output},"cacheRead":0,"cacheWrite":0,"totalTokens":{output},"cost":{{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":{cost}}}}},"stopReason":"stop","timestamp":0}}"#
    )
}

pub(crate) fn user(text: &str) -> String {
    format!(r#"{{"role":"user","content":{text},"timestamp":0,"attribution":"user"}}"#)
}

fn call(id: &str, name: &str, args: &str) -> String {
    format!(r#"{{"type":"toolCall","id":"{id}","name":"{name}","arguments":{args}}}"#)
}

pub(crate) fn words(text: &str) -> String {
    format!(r#"{{"type":"text","text":{}}}"#, quote(text))
}

/// Steps lines into the model, each at its own time.
pub(crate) struct Script<'a> {
    pub hub: &'a mut Hub,
    pub thread: &'static str,
    pub seq: u64,
}

impl Script<'_> {
    pub(crate) fn at(&mut self, at: u64) -> &mut Self {
        step(&mut self.hub.model, Msg::Tick(Millis(at)));
        self
    }

    pub(crate) fn line(&mut self, line: &str) -> &mut Self {
        // A line that does not decode is a scene bug the tests catch by what is missing.
        let _ = play_line(&mut self.hub.model, line);
        self
    }

    pub(crate) fn set(&mut self, body: &str) -> &mut Self {
        let line = update(self.thread, body);
        self.line(&line)
    }

    pub(crate) fn event(&mut self, child: Option<&str>, event: &str) -> &mut Self {
        let seq = self.seq;
        self.seq = seq.saturating_add(1);
        let child = child.map_or_else(String::new, |id| format!(r#","childId":"{id}""#));
        self.set(&format!(
            r#""sessionUpdate":"_yi/event","seq":{seq}{child},"event":{event}"#
        ))
    }

    /// A whole message: started at `from`, ended at `to`.
    pub(crate) fn message(&mut self, from: u64, to: u64, message: &str) -> &mut Self {
        self.at(from)
            .event(
                None,
                &format!(r#"{{"type":"message_start","message":{message}}}"#),
            )
            .at(to)
            .event(
                None,
                &format!(r#"{{"type":"message_end","message":{message}}}"#),
            )
    }

    fn tool(&mut self, at: u64, call: [&str; 3], output: &str, details: &str) -> &mut Self {
        let [id, name, args] = call;
        let end = format!(
            r#"{{"type":"tool_execution_end","toolCallId":"{id}","toolName":"{name}","result":{{"content":[{{"type":"text","text":{}}}],"details":{details}}},"isError":false}}"#,
            quote(output)
        );
        self.at(at)
            .event(
                None,
                &format!(
                    r#"{{"type":"tool_execution_start","toolCallId":"{id}","toolName":"{name}","args":{args}}}"#
                ),
            )
            .at(at.saturating_add(50))
            .event(None, &end)
    }
}

/// The record's options: Supervised, Opus 5.5 at High.
pub(crate) const OPTIONS: &str = r#"[{"configId":"mode","name":"Mode","type":"select","currentValue":"supervised","options":[{"value":"supervised","name":"Supervised"}]},{"configId":"model","name":"Model","type":"select","currentValue":"anthropic/claude-opus-5-5","options":[{"value":"anthropic/claude-opus-5-5","name":"Opus 5.5"}]},{"configId":"thought_level","name":"Effort","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]}]"#;

fn turn_one(s: &mut Script<'_>) {
    s.at(TURN_ONE)
        .line(&recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ))
        .line(&send(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}"#,
        ))
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}","configOptions":{OPTIONS}}}}}"#
        )))
        .set(r#""sessionUpdate":"_yi/name","name":"Rate-limit the upload endpoint""#)
        .set(r#""sessionUpdate":"_yi/workdir","cwd":"~/atlas-api","lane":null"#)
        .set(r#""sessionUpdate":"state_update","state":"running""#);
    s.message(TURN_ONE, TURN_ONE, &user(&quote("Find the upload route")));
    let calls = [
        call("t1", "find", r#"{"pattern":"upload"}"#),
        call("t2", "grep", r#"{"pattern":"post(upload)"}"#),
        call("t3", "read", r#"{"path":"src/routes/upload.rs"}"#),
    ]
    .join(",");
    s.message(TURN_ONE + 200, TURN_ONE + 400, &assistant(&calls, 0, "0"));
    s.tool(
        TURN_ONE + 1_000,
        ["t1", "find", "{}"],
        "src/routes/upload.rs",
        "{}",
    )
    .tool(
        TURN_ONE + 2_000,
        ["t2", "grep", "{}"],
        "src/routes/upload.rs",
        "{}",
    )
    .tool(
        TURN_ONE + 3_000,
        ["t3", "read", "{}"],
        "",
        r#"{"lines":84}"#,
    );
    let found = words("Found the upload route.");
    s.message(
        TURN_ONE + 8_000,
        TURN_ONE + 9_000,
        &assistant(&found, 0, "0.07"),
    )
    .set(r#""sessionUpdate":"state_update","state":"idle""#);
}

fn turn_two(s: &mut Script<'_>) {
    // The server stores what was attached as addresses after the words typed.
    let typed = PROMPT.trim().strip_suffix('"').unwrap_or(PROMPT);
    let prompt = format!(
        "{typed} yi://file/src/routes/upload.rs yi://file/src/middleware/mod.rs yi://file/src/main.rs\""
    );
    const READ: &str = r#"{"path":"src/routes/upload.rs","offset":21,"limit":2}"#;
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
    let read = rows(HUNK, |n| (21..=22).contains(&n));
    let read: Vec<&str> = read.iter().filter_map(|l| l.get(1..)).collect();
    // The mockup draws lines 13 to 15 of the new file, so the edit's patch is that hunk.
    let added = rows(NEW_FILE, |n| (13..=15).contains(&n)).join("\n");
    let made =
        format!("--- /dev/null\n+++ b/src/middleware/rate_limit.rs\n@@ -0,0 +13,3 @@\n{added}\n");
    let changed = rows(HUNK, |_| true).join("\n");
    let edited = format!(
        "--- a/src/routes/upload.rs\n+++ b/src/routes/upload.rs\n@@ -21,4 +21,5 @@\n{changed}\n"
    );
    s.at(TURN_TWO)
        .set(r#""sessionUpdate":"state_update","state":"running""#)
        .set(r#""sessionUpdate":"_yi/plan_progress","plan":{"done":1,"total":3,"running":"Add the limit"}"#);
    s.message(TURN_TWO, TURN_TWO, &user(&prompt))
        .message(TURN_TWO + 100, TURN_TWO + 200, &assistant(&calls, 0, "0"))
        .tool(
            ASKED + 1_000,
            ["c1", "read", READ],
            &read.join("\n"),
            r#"{"lines":84}"#,
        )
        .tool(
            ASKED + 2_000,
            ["c2", "grep", GREP],
            "src/main.rs\nsrc/routes/upload.rs\nsrc/routes/upload.rs",
            r#"{"files":2}"#,
        )
        .tool(
            ASKED + 3_000,
            ["c3", "edit", NEW],
            "",
            &format!(r#"{{"added":48,"removed":0,"patch":{}}}"#, quote(&made)),
        )
        .tool(
            ASKED + 3_050,
            ["c4", "edit", EDIT],
            "",
            &format!(r#"{{"added":6,"removed":1,"patch":{}}}"#, quote(&edited)),
        );
    let answer = format!(
        "{}\n\n#### What changed\n\n- `rate_limit.rs` is new: a token bucket for each user, 20 uploads a minute.\n- `upload.rs`: the limiter now sits in front of the handler.",
        markdown(ANSWER)
    );
    s.message(
        ASKED + 3_200,
        TURN_TWO + 3_400,
        &assistant(&words(&answer), 612, "0.38"),
    )
    .set(r#""sessionUpdate":"usage_update","used":41200,"size":200000"#);
}

/// A child's ask holds the turn; a second waits behind it, and the next answer streams.
fn held(s: &mut Script<'_>) {
    let child = r#""sessionUpdate":"_yi/subagent_update","id":"tests-upload","name":"Tests · upload","status":"running","activity":"waiting","toolUseCount":0,"tokenCount":0,"task":"to check the 429 answer""#;
    let test = call("c5", "bash", r#"{"command":"cargo test -p api upload::"}"#);
    let ask = |id: &str| {
        recv(&format!(
            r#"{{"jsonrpc":"2.0","id":"{id}","method":"session/request_permission","params":{{"sessionId":"{THREAD}","title":"bash requires permission","description":"sandbox on · no network · writes nothing · about 20 s","options":[{{"optionId":"allow_once","name":"Allow once","kind":"allow_once"}},{{"optionId":"allow_always","name":"Always allow cargo test","kind":"allow_always"}},{{"optionId":"reject_once","name":"Reject","kind":"reject_once"}}]}}}}"#
        ))
    };
    s.at(HELD)
        .set(child)
        .event(
            Some("tests-upload"),
            &format!(
                r#"{{"type":"message_end","message":{}}}"#,
                assistant(&test, 0, "0")
            ),
        )
        .line(&ask("perm-1"))
        .event(None, r#"{"type":"permission_requested","toolCallId":"c5","title":"bash requires permission","description":"sandbox on · no network · writes nothing · about 20 s"}"#)
        .set(r#""sessionUpdate":"state_update","state":"requires_action""#)
        .at(HELD + 1_000)
        .line(&ask("perm-2"));
    let writing = format!(
        r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":{}}}}}"#,
        quote(&markdown(WRITING))
    );
    s.at(NOW - 2_000)
        .event(
            None,
            &format!(
                r#"{{"type":"message_start","message":{}}}"#,
                assistant("", 76, "0")
            ),
        )
        .event(None, r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#)
        .event(None, &writing);
}

/// The thread list: this thread, and a fork of it made between its two turns.
fn listed(s: &mut Script<'_>) {
    let row = |id: &str, title: &str, state: &str, last: u64, parent: &str| {
        format!(
            r#"{{"sessionId":"{id}","cwd":"~/atlas-api","title":"{title}","_meta":{{"yi":{{"unseen":0,"lastState":"{state}","lastEventMs":{last},"pinnedMs":null{parent}}}}}}}"#
        )
    };
    let fork = row(
        FORK_ID,
        "Fork · sliding window",
        "idle",
        TURN_ONE + 240_000,
        &format!(r#","parentSessionId":"{THREAD}""#),
    );
    let own = row(
        THREAD,
        "Rate-limit the upload endpoint",
        "requires_action",
        NOW,
        "",
    );
    s.at(NOW)
        .line(&send(
            r#"{"jsonrpc":"2.0","id":3,"method":"session/list","params":{}}"#,
        ))
        .line(&recv(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"result":{{"sessions":[{own},{fork}]}}}}"#
        )));
}

/// The mockup's conversation: turn 1 folded, turn 2 held by a child's ask, a fork between.
pub(crate) fn thread(hub: &mut Hub) {
    let mut s = Script {
        hub,
        thread: THREAD,
        seq: 0,
    };
    turn_one(&mut s);
    turn_two(&mut s);
    held(&mut s);
    listed(&mut s);
}

/// What the extractor saw (`pose=hover`): every detail of every block shown at once.
pub(crate) fn mockup(hub: &mut Hub) {
    thread(hub);
    if let Some(doc) = hub.panes.get_mut::<Doc>(Some(SCENE_PANE)) {
        doc.pose = Pose::Every;
    }
}

/// The pointer resting on the steps line: its pop-over open over the answer.
pub(crate) fn hover(hub: &mut Hub) {
    thread(hub);
    if let Some(doc) = hub.panes.get_mut::<Doc>(Some(SCENE_PANE)) {
        doc.hover = Some(Block::Steps);
    }
}
