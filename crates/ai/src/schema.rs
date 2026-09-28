use serde_json::{Value, json};

const UNSUPPORTED: [&str; 11] = [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
];

pub fn strict(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            if UNSUPPORTED.iter().any(|key| map.contains_key(*key)) {
                return false;
            }
            if map.get("type") == Some(&json!("object")) && !closed(map) {
                return false;
            }
            map.values().all(strict)
        }
        Value::Array(items) => items.iter().all(strict),
        _ => true,
    }
}

fn closed(map: &serde_json::Map<String, Value>) -> bool {
    let Some(Value::Object(properties)) = map.get("properties") else {
        return false;
    };
    let required: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    map.get("additionalProperties") == Some(&Value::Bool(false))
        && properties
            .keys()
            .all(|key| required.contains(&key.as_str()))
}

pub fn anthropic(schema: &Value) -> Option<Value> {
    strict(schema).then(|| json!({"type": "json_schema", "schema": schema}))
}

pub fn chat(schema: &Value) -> Value {
    json!({"type": "json_schema", "json_schema": {"name": "answer", "schema": schema, "strict": strict(schema)}})
}

pub fn responses(schema: &Value) -> Value {
    json!({"type": "json_schema", "name": "answer", "schema": schema, "strict": strict(schema)})
}
