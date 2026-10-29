//! The literal-overlap fact (D170): a number typed into the check that also appears in an
//! artifact was copied, not computed. Reported with every run, never a refusal.

use std::collections::BTreeSet;
use std::path::Path;

use yi_types::spec::Artifact;

/// Fewer significant digits than this is a count or a bound, not a copied answer.
const MIN_SIGNIFICANT: usize = 3;
/// A file past this is data the model reads, not an answer it wrote.
const TEXT_CAP: u64 = 1024 * 1024;

/// Leading and trailing zeros carry no information: `100` and `0.0012` are bounds.
fn significant(token: &str) -> usize {
    let digits: String = token.chars().filter(char::is_ascii_digit).collect();
    digits.trim_start_matches('0').trim_end_matches('0').len()
}

/// Numeric literals in the check: digits with one optional inner `.`, three or more
/// significant digits.
pub fn literals(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut BTreeSet<String>| {
        let candidate = token.trim_end_matches('.').to_owned();
        if !candidate.is_empty() && significant(&candidate) >= MIN_SIGNIFICANT {
            out.insert(candidate);
        }
        token.clear();
    };
    let mut previous: Option<char> = None;
    for c in text.chars() {
        let inside = c.is_ascii_digit() || (c == '.' && !token.is_empty() && !token.contains('.'));
        if inside && (token.is_empty() || previous.is_some_and(|p| p.is_ascii_digit() || p == '.'))
        {
            if token.is_empty() && previous.is_some_and(|p| p.is_ascii_alphanumeric() || p == '_') {
                previous = Some(c);
                continue;
            }
            token.push(c);
        } else {
            flush(&mut token, &mut out);
        }
        previous = Some(c);
    }
    flush(&mut token, &mut out);
    out
}

fn texts(artifact: &Artifact) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![artifact.path().to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                stack.extend(entries.flatten().map(|entry| entry.path()));
            }
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > TEXT_CAP {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((name, text));
        }
    }
    out
}

/// `4213.7 in results.txt` for every check literal found verbatim in an artifact.
pub fn overlaps(check: Option<&Path>, artifacts: &[Artifact]) -> Vec<String> {
    let Some(text) = check.and_then(|path| std::fs::read_to_string(path).ok()) else {
        return Vec::new();
    };
    let literals = literals(&text);
    if literals.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for artifact in artifacts {
        for (name, body) in texts(artifact) {
            for literal in &literals {
                if body.contains(literal.as_str()) {
                    out.push(format!("{literal} in {name}"));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_keep_three_significant_digits_and_skip_identifiers() {
        let found = literals(
            "ok(5, 'R5', abs(v - 4213.7) < 0.01, 'x'); n = 100; y = 0.0012; z = R12; w = 1e3",
        );
        let got: Vec<&str> = found.iter().map(String::as_str).collect();
        assert_eq!(got, vec!["4213.7"]);
    }
}
