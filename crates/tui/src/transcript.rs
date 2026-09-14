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

/// The head says what ran and the tail how it ended, and a command's error is
/// in the tail — keeping ten head lines dropped the half worth reading.
pub(crate) fn preview_lines(text: &str, head: usize, tail: usize) -> Vec<String> {
    let compact = text.len() <= 256 * 1024 && !text.contains('\n');
    let pretty = (compact && text.starts_with(['{', '[']))
        .then(|| serde_json::from_str::<Value>(text).ok())
        .flatten()
        .and_then(|value| serde_json::to_string_pretty(&value).ok());
    let text = pretty.as_deref().unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head.saturating_add(tail) {
        return lines.into_iter().map(str::to_owned).collect();
    }
    let omitted = lines.len().saturating_sub(head.saturating_add(tail));
    lines
        .iter()
        .take(head)
        .map(|line| (*line).to_owned())
        .chain(std::iter::once(format!("… {omitted} more lines")))
        .chain(
            lines
                .iter()
                .skip(lines.len().saturating_sub(tail))
                .map(|line| (*line).to_owned()),
        )
        .collect()
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

/// The edit tool takes only a `patch`; its target lives in the patch's own `[path#TAG]`
/// headers. Without this the cell renders a bare `edit` naming nothing it touched.
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
            if text.chars().count() > 120 {
                text = text.chars().take(120).collect::<String>() + "…";
            }
            text
        }
        None => String::new(),
    }
}

pub(crate) fn intent_of(args: &Value) -> Option<String> {
    args.get("i").and_then(Value::as_str).map(str::to_owned)
}

/// A slice starting inside a fence renders under it reopened; the rail header
/// stays with the slice that opened the block, one block not one per line.
pub(crate) fn paint_slice(
    app: &crate::app::App,
    slice: &str,
) -> (
    Vec<ratatui::text::Line<'static>>,
    Option<crate::highlight::Lang>,
) {
    let width = app
        .content_width()
        .saturating_sub(crate::cell::GUTTER.len());
    let reopen = app.live_reopen.as_ref().filter(|_| !slice.is_empty());
    let source = match reopen {
        Some(open) => format!("{open}\n{slice}"),
        None => slice.to_owned(),
    };
    let mut lang = app.live_lang.clone();
    let lines =
        crate::markdown::render_stream(&source, width, &app.theme, reopen.is_some(), &mut lang);
    (lines, lang)
}
