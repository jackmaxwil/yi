use serde_json::{Map, Value};

use crate::tool::ToolOutput;

pub(super) const READ_LINE_CAP: usize = 2_000;

pub(super) fn integer_argument(
    input: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, String> {
    match input.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .or_else(|| value.as_str()?.parse().ok())
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

/// The requested line windows, 1-based inclusive, merged and clamped. `ranges` and
/// `offset`/`limit` are two spellings of the same thing and never combine.
pub(super) fn read_windows(
    input: &Map<String, Value>,
    explicit_limit: Option<usize>,
    line_count: usize,
) -> Result<Vec<(usize, usize)>, Box<ToolOutput>> {
    let invalid = |message: String| {
        Err(Box::new(crate::tool::error_output_kind(
            message,
            yi_types::event::ToolErrorKind::InvalidArgs,
        )))
    };
    if let Some(ranges) = input.get("ranges") {
        if input.get("offset").is_some() || explicit_limit.is_some() {
            return invalid("pass ranges or offset/limit, not both".to_owned());
        }
        let Some(items) = ranges.as_array() else {
            return invalid("ranges must be an array of [start, end] pairs".to_owned());
        };
        let mut windows: Vec<(usize, usize)> = Vec::new();
        for item in items {
            let pair: Option<(u64, u64)> = item.as_array().and_then(|pair| {
                match (
                    pair.first().and_then(Value::as_u64),
                    pair.get(1).and_then(Value::as_u64),
                ) {
                    (Some(start), Some(end)) if pair.len() == 2 => Some((start, end)),
                    _ => None,
                }
            });
            let Some((start, end)) = pair else {
                return invalid(format!(
                    "range {item} is not a [start, end] pair of positive integers"
                ));
            };
            if start < 1 || end < start {
                return invalid(format!(
                    "range [{start}, {end}] is not ascending and 1-based"
                ));
            }
            let start = usize::try_from(start).unwrap_or(usize::MAX);
            let end = usize::try_from(end).unwrap_or(usize::MAX).min(line_count);
            if start > line_count {
                return invalid(format!(
                    "range start {start} is beyond the file ({line_count} lines)"
                ));
            }
            windows.push((start, end));
        }
        if windows.is_empty() {
            return invalid("ranges is empty".to_owned());
        }
        windows.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in windows {
            match merged.last_mut() {
                Some((_, previous_end)) if start <= previous_end.saturating_add(1) => {
                    *previous_end = (*previous_end).max(end);
                }
                _ => merged.push((start, end)),
            }
        }
        return Ok(merged);
    }
    let offset = match integer_argument(input, "offset") {
        Ok(offset) => offset.map_or(0, |line| line.saturating_sub(1) as usize),
        Err(message) => return invalid(message),
    };
    let limit = explicit_limit.unwrap_or(READ_LINE_CAP);
    let end = offset.saturating_add(limit).min(line_count);
    Ok(vec![(offset + 1, end)])
}
