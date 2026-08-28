use serde_json::Value;
use yi_types::message::{Content, UserContent};

pub(crate) fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// codex `exec_cell::output_lines`: the head says what ran and the tail says
/// how it ended, and a command's error is almost always in the tail. Keeping
/// only the first ten lines dropped exactly the half worth reading.
pub(crate) fn preview_lines(text: &str) -> Vec<String> {
    const HEAD: usize = 5;
    const TAIL: usize = 5;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= HEAD.saturating_add(TAIL) {
        return lines.into_iter().map(str::to_owned).collect();
    }
    let omitted = lines.len().saturating_sub(HEAD.saturating_add(TAIL));
    let mut out: Vec<String> = lines
        .iter()
        .take(HEAD)
        .map(|line| (*line).to_owned())
        .collect();
    out.push(format!("… {omitted} more lines"));
    out.extend(
        lines
            .iter()
            .skip(lines.len().saturating_sub(TAIL))
            .map(|line| (*line).to_owned()),
    );
    out
}

pub(crate) fn thinking_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking { thinking, .. } => Some(thinking.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => text_of(blocks),
    }
}

/// The edit tool takes only a `patch`; its target lives in the patch's own
/// `[path#TAG]` section headers. Without this the cell renders a bare `edit`
/// with no indication of what it touched.
fn patch_targets(patch: &str) -> String {
    let mut paths: Vec<&str> = patch
        .lines()
        .filter_map(|line| line.strip_prefix('[')?.split_once('#'))
        .map(|(path, _)| path)
        .collect();
    paths.dedup();
    match paths.split_first() {
        None => String::new(),
        Some((first, [])) => (*first).to_owned(),
        Some((first, rest)) => format!("{first} +{} more", rest.len()),
    }
}

pub(crate) fn arg_summary(tool: &str, args: &Value) -> String {
    if tool == "edit" {
        let targets = args
            .get("patch")
            .and_then(Value::as_str)
            .map(patch_targets)
            .unwrap_or_default();
        if !targets.is_empty() {
            return targets;
        }
    }
    let arg = match tool {
        "bash" => args.get("cmd").or_else(|| args.get("command")),
        "read" | "edit" | "write" => args.get("path").or_else(|| args.get("file_path")),
        "grep" | "glob" | "find" => args
            .get("pattern")
            .or_else(|| args.get("query"))
            .or_else(|| args.get("glob")),
        "ipython" => args.get("code"),
        "fetch" | "web_search" => args.get("url").or_else(|| args.get("query")),
        _ => None,
    };
    match arg.and_then(Value::as_str) {
        Some(text) => {
            let first = text.lines().next().unwrap_or("");
            let mut text = first.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.chars().count() > 60 {
                text = text.chars().take(60).collect::<String>() + "…";
            }
            text
        }
        None => String::new(),
    }
}

pub(crate) fn intent_of(args: &Value) -> Option<String> {
    args.get("i").and_then(Value::as_str).map(str::to_owned)
}
