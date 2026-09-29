use serde_json::{Value, json};

const UNSUPPORTED: [&str; 19] = [
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
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "patternProperties",
    "dependentSchemas",
    "unevaluatedProperties",
];

pub fn strict(schema: &Value) -> bool {
    is_object(schema) && node(schema)
}

fn is_object(schema: &Value) -> bool {
    schema.get("properties").is_some()
        || match schema.get("type") {
            Some(Value::String(kind)) => kind == "object",
            Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
            _ => false,
        }
}

fn node(schema: &Value) -> bool {
    let Value::Object(map) = schema else {
        return false;
    };
    if UNSUPPORTED.iter().any(|key| map.contains_key(*key)) || (is_object(schema) && !closed(map)) {
        return false;
    }
    let values = |key: &str| match map.get(key) {
        Some(Value::Object(inner)) if key != "items" => inner.values().collect(),
        Some(Value::Array(items)) => items.iter().collect(),
        Some(item @ Value::Object(_)) => vec![item],
        _ => Vec::new(),
    };
    ["properties", "items", "anyOf", "$defs", "definitions"]
        .into_iter()
        .flat_map(values)
        .all(node)
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
