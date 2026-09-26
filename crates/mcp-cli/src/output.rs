use serde_json::Value;

/// `--max-chars` truncation for non-JSON output only; JSON is never
/// truncated — a clipped JSON document is worse than a large one.
pub fn truncate_chars(text: &str, max_chars: Option<usize>) -> String {
    let Some(max) = max_chars else {
        return text.to_owned();
    };
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[... truncated, {} of {} chars shown; use --json for full output ...]",
        &text[..end],
        end,
        text.len()
    )
}

pub fn emit(value: &Value, json: bool, max_chars: Option<usize>) {
    if json {
        println!("{}", serde_json::to_string(value).unwrap_or_default());
        return;
    }
    let text = render_human(value);
    println!("{}", truncate_chars(&text, max_chars));
}

fn render_human(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// Schema snapshot check: `strict` = byte-identical JSON; `compatible`
/// = every expected field present with the same value (server may add).
pub fn schema_compatible(expected: &Value, actual: &Value, strict: bool) -> Result<(), String> {
    if strict {
        if expected == actual {
            return Ok(());
        }
        return Err("schema mismatch (strict)".to_owned());
    }
    fn subset(expected: &Value, actual: &Value, path: &str) -> Result<(), String> {
        match (expected, actual) {
            (Value::Object(exp), Value::Object(act)) => {
                for (key, exp_value) in exp {
                    let act_value = act
                        .get(key)
                        .ok_or_else(|| format!("schema mismatch: missing {path}/{key}"))?;
                    subset(exp_value, act_value, &format!("{path}/{key}"))?;
                }
                Ok(())
            }
            (exp, act) if exp == act => Ok(()),
            _ => Err(format!("schema mismatch at {path}")),
        }
    }
    subset(expected, actual, "")
}
