use serde_json::{Map, Value};

fn escape_control(character: char, repaired: &mut String) {
    match character {
        '\u{8}' => repaired.push_str("\\b"),
        '\u{c}' => repaired.push_str("\\f"),
        '\n' => repaired.push_str("\\n"),
        '\r' => repaired.push_str("\\r"),
        '\t' => repaired.push_str("\\t"),
        other => {
            let code = other as u32;
            repaired.push_str(&format!("\\u{code:04x}"));
        }
    }
}

pub fn repair_json(json: &str) -> String {
    const VALID_ESCAPES: [char; 9] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];
    let characters: Vec<char> = json.chars().collect();
    let mut repaired = String::with_capacity(json.len());
    let mut in_string = false;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if !in_string {
            repaired.push(character);
            if character == '"' {
                in_string = true;
            }
            index = index.saturating_add(1);
            continue;
        }
        if character == '"' {
            repaired.push(character);
            in_string = false;
            index = index.saturating_add(1);
            continue;
        }
        if character == '\\' {
            match characters.get(index.saturating_add(1)) {
                None => {
                    repaired.push_str("\\\\");
                    index = index.saturating_add(1);
                }
                Some('u') => {
                    let digits: String = characters
                        .iter()
                        .skip(index.saturating_add(2))
                        .take(4)
                        .collect();
                    if digits.len() == 4 && digits.chars().all(|d| d.is_ascii_hexdigit()) {
                        repaired.push_str("\\u");
                        repaired.push_str(&digits);
                        index = index.saturating_add(6);
                    } else {
                        repaired.push_str("\\\\");
                        index = index.saturating_add(1);
                    }
                }
                Some(next) if VALID_ESCAPES.contains(next) => {
                    repaired.push('\\');
                    repaired.push(*next);
                    index = index.saturating_add(2);
                }
                Some(_) => {
                    repaired.push_str("\\\\");
                    index = index.saturating_add(1);
                }
            }
            continue;
        }
        if character.is_control() {
            escape_control(character, &mut repaired);
        } else {
            repaired.push(character);
        }
        index = index.saturating_add(1);
    }
    repaired
}

pub fn parse_json_with_repair(json: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(json).or_else(|error| {
        let repaired = repair_json(json);
        if repaired != json {
            serde_json::from_str(&repaired)
        } else {
            Err(error)
        }
    })
}

fn close_partial(json: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for character in json.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut closed = json.trim_end().to_owned();
    if in_string {
        if escaped {
            closed.push('\\');
        }
        closed.push('"');
    }
    for pending in [',', ':'] {
        if closed.ends_with(pending) {
            closed.pop();
        }
    }
    if closed.ends_with(':') {
        closed.push_str("null");
    }
    while let Some(closer) = stack.pop() {
        closed.push(closer);
    }
    Some(closed)
}

/// GLM sometimes leaks its native tool-call markup into the arguments string.
/// A key baked out of `<arg_key>path</arg_key>` reads back as `path`.
fn clean_arg_keys(map: &mut Map<String, Value>) {
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let Some(start) = key.find("<arg_key>") else {
            continue;
        };
        let after = &key[start + "<arg_key>".len()..];
        let Some(end) = after.find("</arg_key>") else {
            continue;
        };
        let real = after[..end].trim().to_owned();
        if real.is_empty() || map.contains_key(&real) {
            continue;
        }
        if let Some(value) = map.remove(&key) {
            map.insert(real, value);
        }
    }
}

/// The same leak with no JSON around it: `<arg_key>k</arg_key><arg_value>v</arg_value>`
/// pairs as the whole arguments string. Both tags must show before it fires.
fn salvage_arg_markup(raw: &str) -> Option<Map<String, Value>> {
    if !raw.contains("<arg_key>") || !raw.contains("<arg_value>") {
        return None;
    }
    let mut out = Map::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<arg_key>") {
        let after_key = &rest[start + "<arg_key>".len()..];
        let Some(key_end) = after_key.find("</arg_key>") else {
            break;
        };
        let key = after_key[..key_end].trim().to_owned();
        let after = &after_key[key_end + "</arg_key>".len()..];
        let Some(value_start) = after.find("<arg_value>") else {
            rest = after;
            continue;
        };
        let after_value = &after[value_start + "<arg_value>".len()..];
        let (text, tail) = match after_value.find("</arg_value>") {
            Some(value_end) => (
                &after_value[..value_end],
                &after_value[value_end + "</arg_value>".len()..],
            ),
            None => (after_value, ""),
        };
        rest = tail;
        if !key.is_empty() {
            out.insert(
                key,
                parse_json_with_repair(text).unwrap_or_else(|_| Value::String(text.to_owned())),
            );
        }
    }
    match out.is_empty() {
        true => None,
        false => Some(out),
    }
}

pub fn parse_streaming_json(partial: &str) -> Map<String, Value> {
    if partial.trim().is_empty() {
        return Map::new();
    }
    let as_object = |value: Value| match value {
        Value::Object(object) => Some(object),
        _ => None,
    };
    if let Ok(value) = parse_json_with_repair(partial)
        && let Some(mut object) = as_object(value)
    {
        clean_arg_keys(&mut object);
        return object;
    }
    for candidate in [close_partial(partial), close_partial(&repair_json(partial))]
        .into_iter()
        .flatten()
    {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate)
            && let Some(mut object) = as_object(value)
        {
            clean_arg_keys(&mut object);
            return object;
        }
    }
    salvage_arg_markup(partial).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_json_parses() {
        let parsed = parse_streaming_json(r#"{"a":1,"b":"x"}"#);
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn truncated_string_closes() {
        let parsed = parse_streaming_json(r#"{"cmd":"cargo te"#); // codespell:ignore te
        assert_eq!(parsed["cmd"], "cargo te"); // codespell:ignore te
    }

    #[test]
    fn truncated_nested_closes() {
        let parsed = parse_streaming_json(r#"{"a":{"b":[1,2"#);
        assert!(parsed.contains_key("a"));
    }

    #[test]
    fn raw_newline_in_string_repairs() {
        let parsed = parse_streaming_json("{\"a\":\"x\ny\"}");
        assert_eq!(parsed["a"], "x\ny");
    }

    #[test]
    fn garbage_yields_empty_object() {
        assert!(parse_streaming_json("not json").is_empty());
    }

    #[test]
    fn bare_arg_markup_reads_back_as_pairs() {
        let parsed = parse_streaming_json(
            "<arg_key>path</arg_key><arg_value>/tmp/x</arg_value><arg_key>verbose</arg_key><arg_value>true</arg_value>",
        );
        assert_eq!(parsed["path"], "/tmp/x");
        assert_eq!(parsed["verbose"], true);
    }

    #[test]
    fn arg_markup_value_that_is_json_parses_as_json() {
        let parsed =
            parse_streaming_json("<arg_key>items</arg_key><arg_value>[\"a\", \"b\"]</arg_value>");
        assert_eq!(parsed["items"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn markup_in_a_json_key_names_the_key() {
        let parsed = parse_streaming_json(r#"{"<arg_key>path</arg_key>": "/tmp/x"}"#);
        assert_eq!(parsed["path"], "/tmp/x");
        assert!(!parsed.contains_key("<arg_key>path</arg_key>"));
    }

    #[test]
    fn arg_markup_text_inside_a_string_value_stays_put() {
        let parsed = parse_streaming_json(r#"{"cmd": "echo <arg_key>x</arg_key>"}"#);
        assert_eq!(parsed["cmd"], "echo <arg_key>x</arg_key>");
    }
}
