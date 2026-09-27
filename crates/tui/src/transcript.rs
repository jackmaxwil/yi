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

/// An assistant message's text blocks, one paragraph each.
pub(crate) fn prose_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The head says what ran and the tail how it ended, and a command's error is
/// in the tail — keeping ten head lines dropped the half worth reading.
pub(crate) fn preview_lines(text: &str, head: usize, tail: usize) -> Vec<String> {
    let compact = text.len() <= 256 * 1024 && !text.contains('\n');
    let pretty = (compact && text.starts_with(|c| "{[".contains(c)))
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

/// `Todos 7/7` from a todo result whose header counts every item done; None while work is open.
pub(crate) fn todo_finished(text: &str) -> Option<String> {
    let counts = text.lines().next()?.strip_prefix("Todos ")?;
    let (done, rest) = counts.split_once('/')?;
    let total = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    (done == total && total.parse::<u64>().ok()? > 0).then(|| format!("Todos {total}/{total}"))
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

/// A slice renders under the context it continues and keeps only its own rows; the rail header
/// stays with the slice that opened the block.
pub(crate) fn paint_slice(
    app: &crate::app::App,
    slice: &str,
) -> (
    Vec<ratatui::text::Line<'static>>,
    Option<crate::highlight::Lang>,
) {
    let width = app
        .content_width()
        .saturating_sub(crate::cell::gutter_cols());
    let mut lang = app.live_lang.clone();
    let theme = &app.theme;
    if slice.is_empty() {
        return (Vec::new(), lang);
    }
    let render = |source: &str, continued: bool, lang: &mut Option<crate::highlight::Lang>| {
        crate::markdown::render_stream(source, width, theme, continued, lang)
    };
    if let Some(open) = &app.live_reopen {
        // An empty code row ends the context, so the slice's rows follow code as in the whole.
        let context = format!("{open}\n\n");
        let drawn = render(&context, true, &mut lang.clone()).len();
        let mut rows = render(&format!("{context}{slice}"), true, &mut lang);
        return (rows.split_off(drawn.min(rows.len())), lang);
    }
    let slice = escaped(slice, app.live_seam.mid);
    let draw = |text: &str| render(text, false, &mut None);
    if let Some(rows) = app
        .live_seam
        .stub
        .as_deref()
        .and_then(|stub| under(stub, &slice, draw))
    {
        return (rows, lang);
    }
    let rows = render(&slice, false, &mut lang);
    (rows, lang)
}

/// A slice cut mid-paragraph is prose; when the parser would read it as another block, the
/// escape says so: pipes outside code in a table's first row, else its first mark.
pub(crate) fn escaped(slice: &str, mid: bool) -> std::borrow::Cow<'_, str> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let block = match Parser::new_ext(slice, options).next() {
        Some(Event::Start(tag)) if mid && !matches!(tag, Tag::Paragraph) => tag,
        _ => return std::borrow::Cow::Borrowed(slice),
    };
    let lead = slice.len() - slice.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = slice.split_at(lead);
    // A backslash escapes only punctuation; before a letter it prints. An ordered marker's
    // mark is its delimiter, after the digits (`2024\. `).
    if !matches!(block, Tag::Table(_)) {
        let mark = rest.trim_start_matches(|ch: char| ch.is_ascii_digit());
        let number = rest.get(..rest.len() - mark.len()).unwrap_or_default();
        return match mark.starts_with(|ch: char| ch.is_ascii_punctuation()) {
            true => std::borrow::Cow::Owned(format!("{indent}{number}\\{mark}")),
            false => std::borrow::Cow::Borrowed(slice),
        };
    }
    let (first, after) = rest.split_at(rest.find('\n').unwrap_or(rest.len()));
    let mut code = false;
    let mut out = String::from(indent);
    for ch in first.chars() {
        code ^= ch == '`';
        if ch == '|' && !code {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push_str(after);
    std::borrow::Cow::Owned(out)
}

/// A slice's own rows under `context`, or `None` when the context's rows do not lead the whole
/// (the slice alone then loses its hang but no word).
pub(crate) fn under(
    context: &str,
    slice: &str,
    render: impl Fn(&str) -> Vec<ratatui::text::Line<'static>>,
) -> Option<Vec<ratatui::text::Line<'static>>> {
    let drawn = render(context);
    let mut rows = render(&format!("{context}{slice}"));
    let text = |row: &ratatui::text::Line<'_>| row.to_string().trim_end().to_owned();
    let leads =
        rows.len() >= drawn.len() && rows.iter().zip(&drawn).all(|(a, b)| text(a) == text(b));
    leads.then(|| rows.split_off(drawn.len()))
}

/// The live tail with open strong, strike and code spans closed, so `**bold wor` renders bold
/// now instead of shifting when its closer arrives. Only a run after a space, before a word.
pub(crate) fn close_spans(tail: &str) -> std::borrow::Cow<'_, str> {
    if crate::markdown::open_fence(tail) {
        return std::borrow::Cow::Borrowed(tail);
    }
    let from = tail.rfind("\n\n").map_or(0, |at| at + 2);
    let chars: Vec<char> = tail.get(from..).unwrap_or_default().chars().collect();
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut code: Option<usize> = None;
    let mut at = 0;
    while let Some(&ch) = chars.get(at) {
        let run = chars
            .get(at..)
            .unwrap_or_default()
            .iter()
            .take_while(|next| **next == ch)
            .count();
        let before = at.checked_sub(1).and_then(|back| chars.get(back)).copied();
        let after = chars.get(at + run).copied();
        let opens = before.is_none_or(char::is_whitespace);
        match (ch, code) {
            ('`', Some(length)) if run == length => code = None,
            ('`', None) if opens && after.is_some_and(|next| !next.is_whitespace()) => {
                code = Some(run);
            }
            ('*' | '_' | '~', None) if run == 2 => {
                if open.last() == Some(&(ch, run))
                    && before.is_some_and(|back| !back.is_whitespace())
                {
                    open.pop();
                } else if opens && after.is_some_and(char::is_alphanumeric) {
                    open.push((ch, run));
                }
            }
            _ => {}
        }
        at += run;
    }
    if open.is_empty() && code.is_none() {
        return std::borrow::Cow::Borrowed(tail);
    }
    // A closer after whitespace is not right-flanking and would print, so it goes before it.
    let body = tail.trim_end();
    let mut closed = body.to_owned();
    if let Some(length) = code {
        closed.push_str(&"`".repeat(length));
    }
    for (ch, run) in open.iter().rev() {
        closed.extend(std::iter::repeat_n(*ch, *run));
    }
    closed.push_str(tail.get(body.len()..).unwrap_or_default());
    std::borrow::Cow::Owned(closed)
}

/// A tail ending inside a fence renders its opener and last `rows` lines, the rows it shows.
/// Incident: a thought's open fence never commits, and all of it cost 2.4 ms a frame at 60 KB.
pub(crate) fn fence_tail(tail: &str, rows: usize) -> std::borrow::Cow<'_, str> {
    let lines: Vec<&str> = tail.split_inclusive('\n').collect();
    let opener = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"));
    match opener {
        Some(at) if crate::markdown::open_fence(tail) && lines.len() - at > rows + 1 => {
            let kept = lines.get(lines.len() - rows..).unwrap_or_default().concat();
            std::borrow::Cow::Owned(format!(
                "{}{kept}",
                lines.get(at).copied().unwrap_or_default()
            ))
        }
        _ => std::borrow::Cow::Borrowed(tail),
    }
}
