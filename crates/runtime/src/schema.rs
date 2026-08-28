use serde_json::Value;

/// A JSON Schema subset: `type`, `required`, `properties`, `items`, `enum`.
/// Anything else in the document is carried to the model and ignored here.
pub struct Schema(Value);

const MAX_DEPTH: u32 = 32;

impl Schema {
    pub fn from_value(value: Value) -> Self {
        Self(value)
    }

    /// Accepts either inline JSON or a path to a JSON file.
    pub fn load(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        if trimmed.starts_with('{') {
            return serde_json::from_str(trimmed)
                .map(Self)
                .map_err(|error| format!("schema is not valid JSON: {error}"));
        }
        let source = std::fs::read_to_string(trimmed)
            .map_err(|error| format!("schema {trimmed}: {error}"))?;
        serde_json::from_str(&source)
            .map(Self)
            .map_err(|error| format!("schema {trimmed} is not valid JSON: {error}"))
    }

    pub fn instruction(&self) -> String {
        let rendered = serde_json::to_string_pretty(&self.0).unwrap_or_else(|_| "{}".to_owned());
        format!(
            "Answer with a single JSON value matching this JSON Schema. Emit the JSON \
             and nothing else: no prose, no code fence.\n{rendered}"
        )
    }

    pub fn validate(&self, value: &Value) -> Result<(), String> {
        check(&self.0, value, "$", 0)
    }
}

fn check(schema: &Value, value: &Value, path: &str, depth: u32) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema nests deeper than {MAX_DEPTH}"));
    }
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(expected) = schema.get("type").and_then(Value::as_str)
        && !matches_type(expected, value)
    {
        return Err(format!(
            "{path}: expected {expected}, found {}",
            kind(value)
        ));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(format!("{path}: {value} is not one of the allowed values"));
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let object = value.as_object();
        for name in required.iter().filter_map(Value::as_str) {
            if object.is_none_or(|fields| !fields.contains_key(name)) {
                return Err(format!("{path}: missing required property {name}"));
            }
        }
    }
    if let (Some(properties), Some(fields)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (name, child_schema) in properties {
            if let Some(child) = fields.get(name) {
                check(
                    child_schema,
                    child,
                    &format!("{path}.{name}"),
                    depth.saturating_add(1),
                )?;
            }
        }
    }
    if let (Some(items), Some(elements)) = (schema.get("items"), value.as_array()) {
        for (index, element) in elements.iter().enumerate() {
            check(
                items,
                element,
                &format!("{path}[{index}]"),
                depth.saturating_add(1),
            )?;
        }
    }
    Ok(())
}

fn matches_type(expected: &str, value: &Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Models fence their JSON however they like; take the first complete value.
pub fn extract(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    let start = trimmed
        .find(['{', '['])
        .ok_or_else(|| "answer contains no JSON value".to_owned())?;
    let candidate = trimmed.get(start..).unwrap_or_default();
    let mut stream = serde_json::Deserializer::from_str(candidate).into_iter::<Value>();
    match stream.next() {
        Some(Ok(value)) => Ok(value),
        Some(Err(error)) => Err(format!("answer is not valid JSON: {error}")),
        None => Err("answer contains no JSON value".to_owned()),
    }
}
