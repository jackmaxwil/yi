//! Test Anything Protocol, the seam between the check and the harness: `ok N - R7 …` binds
//! to a requirement id by the first token of its description, and `# metric k=v` records one.

use std::collections::BTreeMap;

use yi_types::spec::{Finite, ReqId, Status};

/// A diagnostic kept for the model is one line, cut here.
const DIAG_CHARS: usize = 200;

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub status: BTreeMap<ReqId, Status>,
    pub diag: BTreeMap<ReqId, String>,
    pub metrics: BTreeMap<String, Finite>,
    pub unbound: u32,
    /// Whether any TAP test line appeared at all.
    pub any: bool,
}

/// One `ok`/`not ok` line: its verdict, the ids it names, and its description.
fn test_line(line: &str) -> Option<(bool, Vec<ReqId>, String)> {
    let trimmed = line.trim_start();
    let (ok, rest) = if let Some(rest) = trimmed.strip_prefix("not ok") {
        (false, rest)
    } else if let Some(rest) = trimmed.strip_prefix("ok") {
        (true, rest)
    } else {
        return None;
    };
    // `ok` must end the token: `okay` is prose.
    if rest.chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = match rest.split_once(char::is_whitespace) {
        Some((number, tail))
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            tail.trim_start()
        }
        _ if rest.bytes().all(|b| b.is_ascii_digit()) => "",
        _ => rest,
    };
    let rest = rest.strip_prefix("- ").unwrap_or(rest).trim();
    // A directive (`# SKIP`, `# TODO`) means the assertion did not run: not green.
    let (description, directive) = match rest.split_once('#') {
        Some((description, directive)) => (description.trim(), Some(directive.trim())),
        None => (rest, None),
    };
    let ok = ok && directive.is_none_or(|d| d.is_empty());
    let (ids, description) = split_ids(description);
    let text = match directive {
        Some(directive) if !directive.is_empty() => format!("{description} ({directive})"),
        _ => description.to_owned(),
    };
    Some((ok, ids, text))
}

/// The first token binds ids when it is `R7` or `R7,R8`; the description is then the rest.
fn split_ids(description: &str) -> (Vec<ReqId>, &str) {
    let (token, rest) = description
        .split_once(char::is_whitespace)
        .unwrap_or((description, ""));
    let ids: Vec<ReqId> = token
        .split(',')
        .filter_map(|part| ReqId::try_from(part).ok())
        .collect();
    if ids.is_empty() {
        (ids, description)
    } else {
        (ids, rest.trim())
    }
}

fn cut(text: &str) -> String {
    let mut out: String = text.chars().take(DIAG_CHARS).collect();
    if text.chars().count() > DIAG_CHARS {
        out.push('…');
    }
    out
}

#[must_use]
pub fn parse(output: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut last: Option<(bool, Vec<ReqId>)> = None;
    for line in output.lines() {
        if let Some((ok, ids, description)) = test_line(line) {
            parsed.any = true;
            if ids.is_empty() {
                parsed.unbound = parsed.unbound.saturating_add(1);
            }
            for id in &ids {
                let status = parsed.status.entry(id.clone()).or_insert(Status::Green);
                if !ok {
                    *status = Status::Red;
                    parsed
                        .diag
                        .entry(id.clone())
                        .or_insert_with(|| cut(&description));
                }
            }
            last = Some((ok, ids));
            continue;
        }
        let trimmed = line.trim_start();
        let Some(comment) = trimmed.strip_prefix('#') else {
            continue;
        };
        let comment = comment.trim();
        if let Some(metric) = comment.strip_prefix("metric ")
            && let Some((name, value)) = metric.split_once('=')
            && let Ok(value) = value.trim().parse::<f64>()
            && let Ok(value) = Finite::try_from(value)
        {
            parsed.metrics.insert(name.trim().to_owned(), value);
            continue;
        }
        if comment.is_empty() {
            continue;
        }
        if let Some((false, ids)) = &last {
            for id in ids {
                let diag = parsed.diag.entry(id.clone()).or_default();
                if !diag.contains(": ") {
                    let line = cut(&format!("{diag}: {comment}"));
                    *diag = line;
                }
            }
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_parse_binds_ids_and_counts_unbound_lines() {
        let text = "1..4\nok 1 - R1 output written\nnot ok 2 - R2,R3 has legs\n# expected 5 legs, got 4\nok 3 - a line with no id\nok 4 - R4 skipped # SKIP no data\n# metric cost=12.5\n";
        let parsed = parse(text);
        assert!(parsed.any);
        assert_eq!(parsed.unbound, 1);
        let id = |n| ReqId::numbered(n);
        assert_eq!(parsed.status.get(&id(1)), Some(&Status::Green));
        assert_eq!(parsed.status.get(&id(2)), Some(&Status::Red));
        assert_eq!(parsed.status.get(&id(3)), Some(&Status::Red));
        assert_eq!(parsed.status.get(&id(4)), Some(&Status::Red));
        assert_eq!(
            parsed.diag.get(&id(2)).map(String::as_str),
            Some("has legs: expected 5 legs, got 4")
        );
        assert_eq!(
            parsed.diag.get(&id(4)).map(String::as_str),
            Some("skipped (SKIP no data)")
        );
        assert_eq!(parsed.metrics.get("cost").map(|m| m.get()), Some(12.5));
    }

    #[test]
    fn a_red_line_after_a_green_one_for_the_same_id_is_red() {
        let parsed = parse("ok 1 - R1 first\nnot ok 2 - R1 second\n");
        assert_eq!(parsed.status.get(&ReqId::numbered(1)), Some(&Status::Red));
    }

    #[test]
    fn prose_is_not_a_test_line() {
        let parsed = parse("okay then\nnothing to see\nnot okay either\n");
        assert!(!parsed.any);
        assert_eq!(parsed.unbound, 0);
    }
}
