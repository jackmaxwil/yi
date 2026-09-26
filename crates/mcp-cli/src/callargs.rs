use std::io::Read;

use serde_json::{Map, Value};

use crate::args::CallArgs;

/// Httpie-style `key:=value` pairs: values auto-parse as JSON
/// (numbers, booleans, objects, arrays); anything unparseable is a string.
fn parse_pair(pair: &str) -> Result<(String, Value), String> {
    let (key, raw) = pair
        .split_once(":=")
        .ok_or_else(|| format!("argument {pair} is not key:=value, JSON, or stdin"))?;
    if key.is_empty() {
        return Err(format!("argument {pair} has an empty key"));
    }
    let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    Ok((key.to_owned(), value))
}

pub fn resolve(args: &CallArgs) -> Result<Map<String, Value>, String> {
    let parse_object = |text: &str| -> Result<Map<String, Value>, String> {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err("tool arguments must be a JSON object".to_owned()),
            Err(error) => Err(format!("bad JSON arguments: {error}")),
        }
    };
    match args {
        CallArgs::Json(text) => parse_object(text),
        CallArgs::Stdin => {
            let mut text = String::new();
            if std::io::stdin().read_to_string(&mut text).is_err() || text.trim().is_empty() {
                return Ok(Map::new());
            }
            parse_object(&text)
        }
        CallArgs::Pairs(pairs) => {
            let mut map = Map::new();
            for pair in pairs {
                let (key, value) = parse_pair(pair)?;
                map.insert(key, value);
            }
            Ok(map)
        }
    }
}
