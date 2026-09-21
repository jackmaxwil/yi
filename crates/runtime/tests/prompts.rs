//! D166: a prompt example that names something it never defines teaches a name that does
//! not exist (`TASK_SCHEMA` did, for a month).

use std::collections::BTreeSet;

fn code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.starts_with("    ") && !line.trim().is_empty() {
            current.push(line.trim_start());
        } else if line.trim().is_empty() && !current.is_empty() {
            continue;
        } else if !current.is_empty() {
            blocks.push(current.join("\n"));
            current.clear();
        }
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }
    blocks
}

/// The line with its string literals blanked, so a word inside a brief never reads as a name.
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => out.push(' '),
            None if c == '"' || c == '\'' => quote = Some(c),
            None => out.push(c),
        }
    }
    out
}

/// Every ALL_CAPS name a block uses is assigned in that block.
fn undefined_constants(block: &str) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    let mut defined = BTreeSet::new();
    let mut in_brief = false;
    for raw in block.lines() {
        let quotes = raw.matches("\"\"\"").count();
        let inside = in_brief;
        if quotes % 2 == 1 {
            in_brief = !in_brief;
        }
        if inside && quotes == 0 {
            continue;
        }
        // A line that closes a brief is code only after its closing quotes.
        let raw = if inside {
            raw.rsplit("\"\"\"").next().unwrap_or("")
        } else {
            raw
        };
        let line = code_only(raw);
        let line = line.as_str();
        if let Some((name, _)) = line.split_once(" = ")
            && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
        {
            defined.insert(name.to_owned());
        }
        // `shapes.ANSWER` is an attribute of an import, not a name the block owes a definition.
        for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')) {
            let token = token.split('.').next().unwrap_or_default();
            if token.len() > 1 && token.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                used.insert(token.to_owned());
            }
        }
    }
    used.difference(&defined).cloned().collect()
}

#[test]
fn every_prompt_example_defines_the_constants_it_uses() {
    for (name, text) in [
        (
            "orchestrate.md",
            include_str!("../src/prompts/orchestrate.md"),
        ),
        ("doctrine.md", include_str!("../src/prompts/doctrine.md")),
        ("identity.md", include_str!("../src/prompts/identity.md")),
    ] {
        for block in code_blocks(text) {
            let missing = undefined_constants(&block);
            assert!(
                missing.is_empty(),
                "{name}: an example uses {missing:?} without defining it:\n{block}"
            );
        }
    }
}

#[test]
fn the_rot_the_test_exists_for_is_caught() {
    let missing = undefined_constants("r = await h.result(schema=TASK_SCHEMA)");
    assert_eq!(missing.into_iter().collect::<Vec<_>>(), ["TASK_SCHEMA"]);
    assert!(undefined_constants("SCHEMA = {}\nr = await h.result(schema=SCHEMA)").is_empty());
    assert!(undefined_constants("accept = contract(schema(shapes.ANSWER))").is_empty());
    assert_eq!(
        undefined_constants("accept = schema(ANSWER.copy())").len(),
        1
    );
}

/// The runtime's own defaults, so a changed default re-prices every example that leans on it.
const RLM: &str = include_str!("../../../python/yi_runtime/src/rlm/__init__.py");

/// The whole seconds of the first `param: float = N` after `signature` in the rlm runtime.
fn python_default(signature: &str, param: &str) -> u64 {
    RLM.split_once(signature)
        .and_then(|(_, rest)| rest.split_once(&format!("{param}: float = ")))
        .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(u64::MAX)
}

/// The most seconds any `call` in `text` can wait: its `timeout=`, else its first positional
/// number, else `default`.
fn longest(text: &str, call: &str, default: u64) -> u64 {
    text.split(call)
        .skip(1)
        .map(|rest| {
            let args = rest.split(')').next().unwrap_or_default();
            let seconds = |s: &str| s.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
            args.split_once("timeout=")
                .map(|(_, after)| after)
                .or_else(|| {
                    args.starts_with(|c: char| c.is_ascii_digit())
                        .then_some(args)
                })
                .and_then(seconds)
                .unwrap_or(default)
        })
        .max()
        .unwrap_or(0)
}

/// D176: the kernel interrupts a cell at bash's ceiling, and the examples wait on a child and
/// collect it in one cell, so a longer wait reads `[cell aborted]`, never the promised result
/// or `TimeoutError`. Incident: identity.md and the plan skill waited `rlm.wait(120)` then a
/// bare `h.result()`, whose default is 540 s: 660 s in one cell, and neither file was checked.
#[test]
fn no_prompt_example_waits_past_the_cell_ceiling() {
    let wait = python_default("async def wait(timeout", "timeout")
        .min(python_default("async def wait(self, timeout", "timeout"));
    let result = python_default("async def result(\n        self,", "timeout");
    assert_eq!((wait, result), (300, 540), "the defaults this test prices");
    let mut over = Vec::new();
    for (name, text) in [
        (
            "orchestrate.md",
            include_str!("../src/prompts/orchestrate.md"),
        ),
        ("doctrine.md", include_str!("../src/prompts/doctrine.md")),
        ("identity.md", include_str!("../src/prompts/identity.md")),
        (
            "skills/yi/plan/SKILL.md",
            include_str!("../../../skills/yi/plan/SKILL.md"),
        ),
    ] {
        let waited =
            longest(text, "rlm.wait(", wait).saturating_add(longest(text, ".result(", result));
        if waited >= yi_tools::MAX_TIMEOUT_SECS {
            over.push(format!("{name}: an example waits {waited} s in one cell"));
        }
        // F1d: a bounded `plan.run` blocks its cell for the whole budget, spelled in minutes.
        for budget in text.split("budget=\"").skip(1) {
            let minutes = budget
                .split_once("m\"")
                .and_then(|(n, _)| n.parse::<u64>().ok());
            if minutes.is_none_or(|minutes| minutes * 60 >= yi_tools::MAX_TIMEOUT_SECS) {
                over.push(format!(
                    "{name}: a run's budget is not minutes under the ceiling"
                ));
            }
        }
    }
    assert!(over.is_empty(), "{}", over.join("; "));
}
