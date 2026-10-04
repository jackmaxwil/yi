use serde_json::{Map, Value, json};

/// Value bounds strict decoding refuses and a tool's own parser enforces anyway, so a tool schema
/// drops them rather than losing its strict mode.
const BOUNDS: [&str; 11] = [
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

const REFUSED: [&str; 11] = [
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "patternProperties",
    "dependentSchemas",
    "unevaluatedProperties",
    "oneOf",
    "prefixItems",
    "contains",
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
    if BOUNDS
        .iter()
        .chain(&REFUSED)
        .any(|key| map.contains_key(*key))
        || (is_object(schema) && !closed(map))
    {
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

/// A tool schema as strict decoding takes it: every object closed, bounds dropped; with
/// `all_required` (OpenAI) an optional property is required and nullable. None: no closed shape.
pub fn strict_tool(schema: &Value, all_required: bool) -> Option<Value> {
    let Value::Object(map) = schema else {
        return None;
    };
    let mut out = Map::new();
    for (key, value) in map {
        let key = key.as_str();
        if BOUNDS.contains(&key) {
            continue;
        }
        if REFUSED.contains(&key) {
            return None;
        }
        let each = |values: &Map<String, Value>| {
            (values.iter())
                .map(|(name, value)| Some((name.clone(), strict_tool(value, all_required)?)))
                .collect::<Option<Map<String, Value>>>()
        };
        let value = match (key, value) {
            ("properties" | "$defs" | "definitions", Value::Object(values)) => {
                Value::Object(each(values)?)
            }
            ("items", item) => strict_tool(item, all_required)?,
            ("anyOf", Value::Array(options)) => Value::Array(
                (options.iter())
                    .map(|option| strict_tool(option, all_required))
                    .collect::<Option<Vec<Value>>>()?,
            ),
            (_, value) => value.clone(),
        };
        out.insert(key.to_owned(), value);
    }
    if is_object(schema) {
        let required: Vec<Value> = match out.get("required") {
            Some(Value::Array(keys)) => keys.clone(),
            _ => Vec::new(),
        };
        let Some(Value::Object(properties)) = out.get_mut("properties") else {
            return None;
        };
        if all_required {
            let names: Vec<Value> = properties.keys().map(|name| json!(name)).collect();
            for (name, property) in properties.iter_mut() {
                if !required.contains(&json!(name)) {
                    nullable(property);
                }
            }
            out.insert("required".to_owned(), Value::Array(names));
        }
        out.insert("additionalProperties".to_owned(), Value::Bool(false));
    } else if !["type", "anyOf", "enum", "const", "$ref"]
        .iter()
        .any(|key| out.contains_key(*key))
    {
        return None;
    }
    Some(Value::Object(out))
}

fn nullable(node: &mut Value) {
    let Value::Object(map) = node else {
        return;
    };
    match map.get_mut("type") {
        Some(Value::String(kind)) => {
            let kind = kind.clone();
            map.insert("type".to_owned(), json!([kind, "null"]));
        }
        Some(Value::Array(kinds)) if !kinds.contains(&json!("null")) => kinds.push(json!("null")),
        Some(_) => {}
        None => {
            if let Some(Value::Array(options)) = map.get_mut("anyOf") {
                options.push(json!({"type": "null"}));
            }
        }
    }
    if let Some(Value::Array(values)) = map.get_mut("enum")
        && !values.contains(&Value::Null)
    {
        values.push(Value::Null);
    }
}

/// Optional and union-typed properties in a schema, nested ones included: what Anthropic's
/// per-request strict caps count.
pub fn weight(schema: &Value) -> (usize, usize) {
    let Value::Object(map) = schema else {
        return (0, 0);
    };
    let mut total = (0usize, 0usize);
    let mut add = |(optional, unions): (usize, usize)| {
        total = (
            total.0.saturating_add(optional),
            total.1.saturating_add(unions),
        );
    };
    if let Some(Value::Object(properties)) = map.get("properties") {
        let required = map.get("required").and_then(Value::as_array);
        for (name, property) in properties {
            let optional = !required.is_some_and(|keys| keys.contains(&json!(name)));
            let union = property.get("anyOf").is_some()
                || property.get("type").is_some_and(Value::is_array);
            add((usize::from(optional), usize::from(union)));
            add(weight(property));
        }
    }
    if let Some(item) = map.get("items") {
        add(weight(item));
    }
    for option in map
        .get("anyOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add(weight(option));
    }
    total
}

/// A null a strict call sends for an optional property means absent: every tool reads the key's
/// absence, so a null is dropped at any object depth before the call reaches it.
pub(crate) fn drop_nulls(arguments: &mut Map<String, Value>) {
    arguments.retain(|_, value| !value.is_null());
    for value in arguments.values_mut() {
        match value {
            Value::Object(inner) => drop_nulls(inner),
            Value::Array(items) => {
                for inner in items.iter_mut().filter_map(Value::as_object_mut) {
                    drop_nulls(inner);
                }
            }
            _ => {}
        }
    }
}
