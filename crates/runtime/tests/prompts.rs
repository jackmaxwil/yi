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
        for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
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
}

/// The most seconds `key` is followed by anywhere in `text`.
fn longest(text: &str, key: &str) -> u64 {
    text.split(key)
        .skip(1)
        .filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .filter_map(|digits| digits.parse().ok())
        .max()
        .unwrap_or(0)
}

/// D176: the kernel interrupts a cell at bash's ceiling, and the examples wait on a child and
/// collect it in one cell, so a longer wait reads `[cell aborted]`, never the promised result
/// or `TimeoutError`.
#[test]
fn no_prompt_example_waits_past_the_cell_ceiling() {
    let text = include_str!("../src/prompts/doctrine.md");
    let waited = longest(text, "rlm.wait(").saturating_add(longest(text, "timeout="));
    assert!(
        waited < yi_tools::MAX_TIMEOUT_SECS,
        "doctrine.md: an example waits {waited} s in one cell"
    );
}
