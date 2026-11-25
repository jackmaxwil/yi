//! A long, real-shaped conversation as wire lines, for the feel measurements.

use yd_core::ids::Millis;
use yd_core::msg::{Msg, step};
use yd_core::wire::replay::play;
use yd_ui::win::Win;
use yi_desktop::root;

pub const TURN: &str = include_str!("../../../../fixtures/wire/turn.jsonl");

/// One answer as a model writes it: prose, a list, inline code, a fenced block and a table.
pub const ANSWER: &str = "## What changed\n\nThe reducer now folds each `text_delta` into the open message **in place**, so a long answer costs the same on its last word as on its first. Three things follow from that:\n\n1. The view reads `Transcript::turns` once a frame, not once a message.\n2. A settled answer keeps its parsed blocks; only the open one is parsed again.\n3. Scrolling never runs the view.\n\n```rust\npub fn fold(model: &mut Model, delta: &str) -> Result<(), FoldError> {\n    let open = model.open_message().ok_or(FoldError::NoOpenMessage)?;\n    open.text.push_str(delta);\n    open.version = open.version.saturating_add(1);\n    Ok(())\n}\n```\n\n| Path | Before | After |\n|---|---|---|\n| `fold` | 41 µs | 3 µs |\n| `view` | 9.2 ms | 0.6 ms |\n| `paint` | 2.1 ms | 2.0 ms |\n\nThe remaining cost is paint, which re-emits every visible glyph; that is the framework's and is tracked there. Nothing else in the frame scales with the length of the thread. Run `just budgets` to see the numbers on this machine, and `just feel` for the long conversation.\n";

fn json(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// Line `at` (1-based) of the recorded turn, as message `n` of the thread, saying `words`.
pub fn line(at: usize, n: usize, seq: usize, words: &str) -> String {
    let template = TURN.lines().nth(at.saturating_sub(1)).unwrap_or_default();
    let mut text = template
        .replace("faux: ", &json(words))
        .replace("\"hello\"", &format!("\"{}\"", json(words)))
        .replace("msg_p1", &format!("msg_p{n}"))
        .replace("\"msg_1\"", &format!("\"msg_{n}\""));
    if let Some(found) = text.find("\"seq\":") {
        let (head, tail) = text.split_at(found.saturating_add(6));
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        text = format!("{head}{seq}{}", tail.get(digits..).unwrap_or_default());
    }
    text
}

pub fn feed(win: &Win, text: &str) -> Result<(), String> {
    let mut hub = win.hub.try_borrow_mut().map_err(|e| e.to_string())?;
    play(&mut hub.model, text).map_err(|e| e.to_string())?;
    Ok(())
}

/// A window's state with the clock set and nothing connected.
pub fn fresh() -> Result<Win, String> {
    let win = root::init();
    let mut hub = win.hub.try_borrow_mut().map_err(|e| e.to_string())?;
    step(&mut hub.model, Msg::Tick(Millis(1_794_679_124_000)));
    drop(hub);
    Ok(win)
}

/// `turns` turns of long answers, the last holding `answers` of them and left running; the
/// second and third values are the last sequence number and message number.
pub fn answers(turns: usize, answers: usize) -> (String, usize, usize) {
    let mut lines: Vec<String> = TURN.lines().take(4).map(str::to_owned).collect();
    // Invariant: the reducer drops an event whose number does not follow the last, so only a
    // line that carries one takes the next, from zero.
    let (mut next, mut message) = (0usize, 0usize);
    let mut push = |at: usize, message: usize, lines: &mut Vec<String>| {
        lines.push(line(
            at,
            message,
            next,
            if at < 16 { PROMPT } else { ANSWER },
        ));
        let numbered = TURN.lines().nth(at.saturating_sub(1));
        if numbered.is_some_and(|recorded| recorded.contains("\"seq\":")) {
            next = next.saturating_add(1);
        }
    };
    for turn in 0..turns {
        let last = turn.saturating_add(1) == turns;
        message = message.saturating_add(1);
        for at in [9, 10, 11, 12, 13, 15] {
            push(at, message, &mut lines);
        }
        for _ in 0..(if last { answers } else { 1 }) {
            message = message.saturating_add(1);
            for at in [17, 18, 19, 21, 22] {
                push(at, message, &mut lines);
            }
        }
        if !last {
            for at in [25, 26, 27] {
                push(at, message, &mut lines);
            }
        }
    }
    (lines.join("\n"), next.saturating_sub(1), message)
}

const PROMPT: &str = "Fold the deltas in place and show me the numbers";

pub const TOOLS: &str = include_str!("../../../../fixtures/wire/tools-turn.jsonl");

/// The recorded eight-tool turn `repeats` times over as one running turn of the thread.
pub fn tool_lines(repeats: usize) -> (String, usize) {
    heavy_tool_lines(repeats, 0)
}

/// [`tool_lines`] with every tool's output grown by `weight` bytes of source lines, as a
/// read of a real file is.
pub fn heavy_tool_lines(repeats: usize, weight: usize) -> (String, usize) {
    let source = "    let open = model.open_message().ok_or(FoldError::NoOpenMessage)?;\\n";
    let pad = source.repeat(weight / source.len().saturating_sub(1).max(1));
    let session = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    let mut out: Vec<String> = TURN.lines().take(4).map(str::to_owned).collect();
    let (mut last, mut next) = (0usize, 0usize);
    for r in 0..repeats {
        for recorded in TOOLS.lines().take(205).skip(4) {
            let user = recorded.contains("\"role\":\"user\"") && !recorded.contains("agent_end");
            let prompt = recorded.contains("session/prompt")
                || recorded.contains("\"user_message\"")
                || recorded.contains("\"id\":3,");
            if r > 0 && (user || prompt) {
                continue;
            }
            let mut text = recorded.replace("01a1e6a3-5774-78cb-a816-e2b6870929a0", session);
            for k in 1..=8 {
                text = text.replace(&format!("\"t{k}\""), &format!("\"t{r}x{k}\""));
                text = text.replace(&format!("_{k}\""), &format!("_{r}x{k}\""));
            }
            text = text.replace("\"msg_9\"", &format!("\"msg_{r}x9\""));
            // Invariant: the reducer drops an event whose number does not follow the last.
            if let Some(found) = text.rfind("\"seq\":") {
                let (head, tail) = text.split_at(found.saturating_add(6));
                let digits = tail.chars().take_while(char::is_ascii_digit).count();
                let rest = tail.get(digits..).unwrap_or_default();
                // An event's own number closes its line; one inside what a tool printed does not.
                if rest == "}}}}" {
                    last = next;
                    next = next.saturating_add(1);
                    text = format!("{head}{last}{rest}");
                }
            }
            if recorded.contains("\"tool_execution_end\"") {
                text = text.replacen("\"text\":\"", &format!("\"text\":\"{pad}"), 1);
            }
            out.push(text);
        }
    }
    (out.join("\n"), last)
}
