use serde_json::Value;

/// A JSON Schema subset: `type`, `required`, `properties`, `items`, `enum`. An instruction to
/// a model carries anything else and ignores it; a contract criterion refuses it instead.
pub struct Schema(Value);

const MAX_DEPTH: u32 = 32;

/// What a criterion may carry: the five keywords this subset decides, then the descriptive ones
/// that assert nothing. An allowlist, so an unimplemented keyword is refused, never ignored.
const CRITERION_KEYWORDS: [&str; 12] = [
    "type",
    "required",
    "properties",
    "items",
    "enum",
    "description",
    "title",
    "examples",
    "default",
    "$comment",
    "$schema",
    "$id",
];

impl Schema {
    pub fn from_value(value: Value) -> Result<Self, String> {
        shape(&value, "$", 0, false)?;
        Ok(Self(value))
    }

    /// # Errors
    /// Malformed, or carrying a keyword this subset does not implement and so cannot decide.
    pub fn criterion(value: Value) -> Result<Self, String> {
        shape(&value, "$", 0, true)?;
        Ok(Self(value))
    }

    /// Accepts either inline JSON or a path to a JSON file.
    pub fn load(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        if trimmed.starts_with('{') {
            let value = serde_json::from_str(trimmed)
                .map_err(|error| format!("schema is not valid JSON: {error}"))?;
            return Self::from_value(value);
        }
        let source = std::fs::read_to_string(trimmed)
            .map_err(|error| format!("schema {trimmed}: {error}"))?;
        let value = serde_json::from_str(&source)
            .map_err(|error| format!("schema {trimmed} is not valid JSON: {error}"))?;
        Self::from_value(value)
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

fn shape(schema: &Value, path: &str, depth: u32, criterion: bool) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema nests deeper than {MAX_DEPTH}"));
    }
    let Some(object) = schema.as_object() else {
        return Err(format!(
            "{path}: expected a schema object, found {}",
            kind(schema)
        ));
    };
    if criterion && let Some(name) = object.keys().find(|name| !known_keyword(name)) {
        return Err(format!(
            "{path}.{name}: this schema subset does not implement {name}, so a criterion \
             may not assert it"
        ));
    }
    if let Some(declared) = object.get("type")
        && !declared.as_str().is_some_and(|name| {
            matches!(
                name,
                "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
            )
        })
    {
        return Err(format!("{path}.type: {declared} is not a JSON Schema type"));
    }
    if let Some(required) = object.get("required")
        && !required
            .as_array()
            .is_some_and(|names| names.iter().all(Value::is_string))
    {
        return Err(format!(
            "{path}.required: expected an array of property names, found {required}"
        ));
    }
    if let Some(allowed) = object.get("enum")
        && !allowed.is_array()
    {
        return Err(format!(
            "{path}.enum: expected an array of values, found {}",
            kind(allowed)
        ));
    }
    match object.get("properties") {
        Some(Value::Object(properties)) => {
            for (name, child) in properties {
                shape(
                    child,
                    &format!("{path}.properties.{name}"),
                    depth.saturating_add(1),
                    criterion,
                )?;
            }
        }
        Some(other) => {
            return Err(format!(
                "{path}.properties: expected an object, found {}",
                kind(other)
            ));
        }
        None => {}
    }
    match object.get("items") {
        Some(items) => shape(
            items,
            &format!("{path}.items"),
            depth.saturating_add(1),
            criterion,
        ),
        None => Ok(()),
    }
}

fn check(schema: &Value, value: &Value, path: &str, depth: u32) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema nests deeper than {MAX_DEPTH}"));
    }
    let Some(schema) = schema.as_object() else {
        return Err(format!("{path}: schema is not an object"));
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

fn known_keyword(keyword: &str) -> bool {
    CRITERION_KEYWORDS.contains(&keyword)
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
        _ => false,
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

/// Models fence their JSON however they like; when the reply does not parse whole, the
/// last fenced block that parses as JSON is the answer, and prose (including quoted JSON
/// and bracketed tags) around it never reaches the parser. Inline JSON is the fallback.
pub fn extract(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    if let Some(value) = last_fenced_json(trimmed) {
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

fn last_fenced_json(text: &str) -> Option<Value> {
    let mut last = None;
    let mut content = String::new();
    let mut fenced = false;
    for line in text.lines() {
        let bare = line.trim_start();
        if fenced {
            if bare.starts_with("```") {
                if let Ok(value) = serde_json::from_str::<Value>(content.trim()) {
                    last = Some(value);
                }
                fenced = false;
                content.clear();
            } else {
                content.push_str(line);
                content.push('\n');
            }
        } else if bare.starts_with("```") {
            fenced = true;
        }
    }
    last
}
