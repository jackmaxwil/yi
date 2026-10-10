//! Stage 9 line selection: after the reducer cut a bash output, a cheap model names the line
//! ranges of the full output that explain it, and those lines ride verbatim after the cut.

use std::sync::Arc;
use std::time::Duration;

use yi_loop::interrupt::InterruptSignal;
use yi_types::event::ToolResult;
use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{LlmContext, Model};

use crate::provider::ProviderStream;

/// A selector slower than this costs the turn more than its lines are worth.
const DEADLINE: Duration = Duration::from_secs(5);
const LINES_IN: usize = 4_000;
const LINES_OUT: usize = 200;

pub struct Selector {
    provider: Arc<ProviderStream>,
    model: Model,
}

impl Selector {
    pub fn new(provider: Arc<ProviderStream>, model: Model) -> Self {
        Self { provider, model }
    }
}

fn schema() -> serde_json::Value {
    let range = serde_json::json!({"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2});
    serde_json::json!({
        "type": "object",
        "properties": {"ranges": {"type": "array", "items": range}},
        "required": ["ranges"],
        "additionalProperties": false
    })
}

/// The model's 1-based inclusive ranges over `lines`, those in bounds only, at most
/// [`LINES_OUT`] lines in all; the reason when it gave none.
pub async fn select_ranges(
    selector: &Selector,
    command: &str,
    exit: Option<i64>,
    lines: &[&str],
) -> (Result<Vec<(usize, usize)>, String>, Usage) {
    let numbered: Vec<String> = (lines.iter().enumerate())
        .map(|(at, line)| format!("{}| {line}", at + 1))
        .collect();
    let exit = exit.map_or_else(
        || "with no exit code".to_owned(),
        |code| format!("with exit code {code}"),
    );
    let prompt = format!(
        "The command `{command}` finished {exit}. Its output, one numbered line each:\n{}\n\nReturn the 1-based inclusive line ranges a coding agent needs to understand the outcome: every failure, error, assertion and summary, with the lines that locate them. At most {LINES_OUT} lines in all; ranges only.",
        numbered.join("\n")
    );
    let context = LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "You pick the lines of a command's output that matter. You answer with line ranges only.".to_owned(),
        messages: vec![AgentMessage::task(&prompt, 0)],
        transient: Vec::new(),
        schema: Some(schema()),
        shared_through: None,
        reuse: yi_types::model::Reuse::OneShot,
        tools: None,
        tool_choice: None,
    };
    let effort = selector.model.clamp_effort(yi_types::model::Effort::Off);
    let signal = InterruptSignal::default();
    let call = crate::compaction::complete_text(
        &selector.provider,
        &selector.model,
        &context,
        effort,
        &signal,
    );
    let (reply, usage) = match tokio::time::timeout(DEADLINE, call).await {
        Ok(answered) => answered,
        Err(_) => {
            return (
                Err(format!("no answer in {} s", DEADLINE.as_secs())),
                Usage::zero(),
            );
        }
    };
    let ranges = reply.and_then(|text| parse(&text, lines.len()));
    (ranges, usage)
}

fn parse(text: &str, total: usize) -> Result<Vec<(usize, usize)>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|error| format!("not JSON: {error}"))?;
    let pairs = value
        .get("ranges")
        .and_then(serde_json::Value::as_array)
        .ok_or("no `ranges`")?;
    let mut ranges: Vec<(usize, usize)> = pairs
        .iter()
        .filter_map(|pair| {
            let at = |index: usize| usize::try_from(pair.get(index)?.as_u64()?).ok();
            Some((at(0)?, at(1)?))
        })
        .filter(|(from, to)| (1..=*to).contains(from) && *to <= total)
        .collect();
    ranges.sort_unstable();
    let mut kept = Vec::new();
    let mut budget = LINES_OUT;
    for (from, to) in ranges {
        let to = to.min(from.saturating_add(budget).saturating_sub(1));
        if budget == 0 || to < from {
            break;
        }
        budget = budget.saturating_sub(to.saturating_add(1).saturating_sub(from));
        kept.push((from, to));
    }
    match kept.is_empty() {
        true => Err("no range inside the output".to_owned()),
        false => Ok(kept),
    }
}

/// The full output a cut result points at, when the reducer or the capture cut it.
fn full_output(text: &str) -> Option<&str> {
    let cut = text.contains(" lines omitted: ") || text.contains("[capture cut: ");
    let pointer = text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("[full output")?.strip_suffix(']'));
    pointer
        .and_then(|rest| rest.split_once(": "))
        .map(|(_, path)| path)
        .filter(|_| cut)
}

/// The selected lines of a cut bash result's full output, appended under a row naming the model
/// and the ranges; a selector that gives none says why, and the cut stands alone.
pub(crate) async fn widen(
    selector: &Selector,
    store: Option<yi_session::SharedSession>,
    command: &str,
    result: &mut ToolResult,
) {
    let text = yi_types::message::join_text(&result.content, "\n");
    let Some(path) = full_output(&text) else {
        return;
    };
    let Ok(full) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = full.lines().collect();
    let model = selector.model.id.as_str();
    if lines.len() > LINES_IN {
        let row = format!(
            "[models.selector skipped: the full output has {} lines, past LINES_IN {LINES_IN}]",
            lines.len()
        );
        crate::affordance::append(result, &row);
        return;
    }
    let exit = result
        .details
        .get("exitCode")
        .and_then(serde_json::Value::as_i64);
    let (ranges, usage) = select_ranges(selector, command, exit, &lines).await;
    if let Some(store) = &store {
        // A lost cost row must not cost the call its lines.
        crate::spend::book_side_call(store, "side:select", usage).ok();
    }
    let ranges = match ranges {
        Ok(ranges) => ranges,
        Err(reason) => {
            let row = format!(
                "[models.selector {model} gave no lines ({reason}); the cut above is the reducer's alone]"
            );
            crate::affordance::append(result, &row);
            return;
        }
    };
    let named: Vec<String> = ranges
        .iter()
        .map(|(from, to)| format!("{from}-{to}"))
        .collect();
    let shown: std::collections::HashSet<&str> = text.lines().collect();
    let picked: Vec<&str> = (ranges.iter())
        .flat_map(|(from, to)| lines.get(from.saturating_sub(1)..*to).unwrap_or_default())
        .copied()
        .filter(|line| !shown.contains(line))
        .collect();
    let mut block = vec![format!(
        "[lines {} of the full output, selected by {model} (models.selector); {} not already shown above]",
        named.join(", "),
        picked.len()
    )];
    block.extend(picked.iter().map(|line| (*line).to_owned()));
    crate::affordance::append(result, &block.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::{full_output, parse};

    #[test]
    fn ranges_out_of_bounds_are_dropped_and_the_total_is_capped() {
        let text = r#"{"ranges": [[0, 2], [5, 3], [8, 12], [3, 4], [9000, 9001]]}"#;
        assert_eq!(parse(text, 10), Ok(vec![(3, 4)]));
        let wide = r#"{"ranges": [[1, 150], [151, 400]]}"#;
        assert_eq!(parse(wide, 1000), Ok(vec![(1, 150), (151, 200)]));
        assert!(parse("the failure is on line 4", 10).is_err());
    }

    #[test]
    fn only_a_cut_result_names_its_full_output() {
        let cut = "a\n[3 lines omitted: 2-4]\nz\n[full output: /tmp/x.txt]";
        assert_eq!(full_output(cut), Some("/tmp/x.txt"));
        let clipped =
            "a\n[capture cut: kept 1 of 2 bytes]\n[full output, first 256 MiB: /tmp/y.txt]";
        assert_eq!(full_output(clipped), Some("/tmp/y.txt"));
        assert_eq!(full_output("whole\n[full output: /tmp/z.txt]"), None);
    }
}
