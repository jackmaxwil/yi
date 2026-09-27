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

pub fn parse_streaming_json(partial: &str) -> Map<String, Value> {
    if partial.trim().is_empty() {
        return Map::new();
    }
    let as_object = |value: Value| match value {
        Value::Object(object) => Some(object),
        _ => None,
    };
    if let Ok(value) = parse_json_with_repair(partial)
        && let Some(object) = as_object(value)
    {
        return object;
    }
    for candidate in [close_partial(partial), close_partial(&repair_json(partial))]
        .into_iter()
        .flatten()
    {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate)
            && let Some(object) = as_object(value)
        {
            return object;
        }
    }
    Map::new()
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
}
