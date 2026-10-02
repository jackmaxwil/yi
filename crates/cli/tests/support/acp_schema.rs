//! A strict reader of the vendored upstream ACP v2 schema: a frame Yi emits passes only if a
//! typed client built on that schema would accept it and the extensibility rules hold.
use std::collections::HashSet;
use std::error::Error;

use serde_json::{Map, Value};

/// Invariant: this file is the upstream `schema/v2/schema.json` at the named tag, byte for
/// byte; a bump adds the next tag's file beside it and changes these two constants.
const PINNED: &str = "schema-v2.0.0-alpha.5.json";
const PINNED_SHA256: &str = "3df13661962bf9ed3162a3e50d75fab3d768247995bfb1754b0d844ae139ce4c";

pub struct Schema {
    defs: Map<String, Value>,
}

#[derive(Default)]
struct Seen {
    props: HashSet<String>,
    closed: bool,
    open: bool,
}

impl Schema {
    pub fn pinned() -> Result<Self, Box<dyn Error>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/acp-v2")
            .join(PINNED);
        let bytes = std::fs::read(&path)?;
        let digest = yi_types::plan::canonical::Digest::of(&bytes).hex();
        if digest != PINNED_SHA256 {
            return Err(format!("{PINNED} was edited: sha256 {digest}").into());
        }
        let mut root: Value = serde_json::from_slice(&bytes)?;
        let defs = match root.get_mut("$defs").map(Value::take) {
            Some(Value::Object(defs)) => defs,
            _ => return Err("the schema has no $defs".into()),
        };
        Ok(Self { defs })
    }

    /// The def a frame answers to, found by the schema's own `x-method` and `x-side` tags.
    pub fn def_for(&self, method: &str, suffix: &str) -> Option<&str> {
        self.defs.iter().find_map(|(name, def)| {
            (def.get("x-method").and_then(Value::as_str) == Some(method) && name.ends_with(suffix))
                .then_some(name.as_str())
        })
    }

    pub fn check(&self, def: &str, value: &Value) -> Result<(), String> {
        let schema = self.defs.get(def).ok_or_else(|| format!("no def {def}"))?;
        self.node(schema, value, "")
            .map_err(|error| format!("{def} {error}"))
    }

    /// Invariant: the spec forbids custom root fields, so an object's keys must all be named
    /// by the subschemas that applied to it, unless one of them leaves it open.
    fn node(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let seen = self.apply(schema, value, at)?;
        if let Value::Object(map) = value
            && seen.closed
            && !seen.open
            && let Some(key) = map.keys().find(|key| !seen.props.contains(*key))
        {
            return Err(format!("{at}: custom root field {key:?}"));
        }
        Ok(())
    }

    fn apply(&self, schema: &Value, value: &Value, at: &str) -> Result<Seen, String> {
        let mut seen = Seen::default();
        let Some(schema) = schema.as_object() else {
            return Ok(seen);
        };
        for (keyword, rule) in schema {
            match keyword.as_str() {
                "$ref" => {
                    let name = rule
                        .as_str()
                        .and_then(|reference| reference.strip_prefix("#/$defs/"))
                        .ok_or_else(|| format!("{at}: unresolvable $ref {rule}"))?;
                    let def = self
                        .defs
                        .get(name)
                        .ok_or_else(|| format!("{at}: no def {name}"))?;
                    merge(&mut seen, self.apply(def, value, at)?);
                }
                "allOf" => {
                    for part in rule.as_array().into_iter().flatten() {
                        merge(&mut seen, self.apply(part, value, at)?);
                    }
                }
                "anyOf" => merge(&mut seen, self.any_of(rule, value, at)?),
                "not" => {
                    if self.apply(rule, value, at).is_ok() {
                        return Err(format!("{at}: matches a `not` arm"));
                    }
                }
                "const" if value != rule => {
                    return Err(format!("{at}: {value} is not {rule}"));
                }
                "enum" if !rule.as_array().is_some_and(|all| all.contains(value)) => {
                    return Err(format!("{at}: {value} is not one of {rule}"));
                }
                "type" => typed(rule, value, at)?,
                "required" => {
                    let keys = rule.as_array().into_iter().flatten();
                    for key in keys.filter_map(Value::as_str) {
                        if value.is_object() && value.get(key).is_none() {
                            return Err(format!("{at}: missing required {key:?}"));
                        }
                    }
                }
                "properties" => {
                    seen.closed = true;
                    for (key, property) in rule.as_object().into_iter().flatten() {
                        seen.props.insert(key.clone());
                        if let Some(field) = value.get(key) {
                            self.node(property, field, &format!("{at}/{key}"))?;
                        }
                    }
                }
                "additionalProperties" => seen.open |= rule == &Value::Bool(true),
                "items" => {
                    for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                        self.node(rule, item, &format!("{at}/{index}"))?;
                    }
                }
                "minItems" => {
                    let floor = rule.as_u64().unwrap_or(0);
                    let len = value.as_array().map_or(0, Vec::len);
                    let len = u64::try_from(len).unwrap_or(u64::MAX);
                    if value.is_array() && len < floor {
                        return Err(format!("{at}: {len} items, at least {floor}"));
                    }
                }
                "minimum" | "maximum" => bound(keyword, rule, value, at)?,
                "pattern" | "oneOf" | "unevaluatedProperties" => {
                    return Err(format!("{at}: the validator does not implement {keyword}"));
                }
                _ => {}
            }
        }
        Ok(seen)
    }

    /// Invariant: an `other` arm is the extension slot, and extensions may only use
    /// `_`-prefixed values; a bare unknown value is reserved for future ACP versions.
    fn any_of(&self, arms: &Value, value: &Value, at: &str) -> Result<Seen, String> {
        let mut seen = Seen::default();
        let mut matched = false;
        let mut misses = Vec::new();
        for arm in arms.as_array().into_iter().flatten() {
            let result = self.apply(arm, value, at).and_then(|arm_seen| {
                let title = arm.get("title").and_then(Value::as_str);
                if matches!(title, Some("other" | "unknown"))
                    && let Some(tag) = discriminator(arm, value)
                    && !tag.starts_with('_')
                {
                    return Err(format!("{at}: {tag:?} squats an ACP-reserved value"));
                }
                Ok(arm_seen)
            });
            match result {
                Ok(arm_seen) => {
                    matched = true;
                    merge(&mut seen, arm_seen);
                }
                Err(miss) => misses.push((claims(arm, value), miss)),
            }
        }
        if matched {
            return Ok(seen);
        }
        // The arm whose tag the value carries is the one it meant; else the deepest miss.
        misses.sort_by_key(|(claimed, miss)| {
            let depth = miss
                .split(':')
                .next()
                .map_or(0, |path| path.matches('/').count());
            std::cmp::Reverse((*claimed, depth, miss.len()))
        });
        Err(misses
            .into_iter()
            .next()
            .map_or_else(|| format!("{at}: no arm matched"), |(_, miss)| miss))
    }
}

fn merge(into: &mut Seen, from: Seen) {
    into.props.extend(from.props);
    into.closed |= from.closed;
    into.open |= from.open;
}

fn claims(arm: &Value, value: &Value) -> bool {
    let Some(key) = arm.pointer("/required/0").and_then(Value::as_str) else {
        return false;
    };
    arm.pointer(&format!("/properties/{key}/const"))
        .is_some_and(|tag| value.get(key) == Some(tag))
}

fn discriminator<'a>(arm: &Value, value: &'a Value) -> Option<&'a str> {
    if let Some(tag) = value.as_str() {
        return Some(tag);
    }
    let key = arm.pointer("/required/0")?.as_str()?;
    value.get(key)?.as_str()
}

fn typed(rule: &Value, value: &Value, at: &str) -> Result<(), String> {
    let fits = |name: &str| match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    };
    let ok = match rule {
        Value::String(name) => fits(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).any(fits),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{at}: {value} is not {rule}"))
    }
}

fn bound(keyword: &str, rule: &Value, value: &Value, at: &str) -> Result<(), String> {
    let (Some(limit), Some(number)) = (rule.as_f64(), value.as_f64()) else {
        return Ok(());
    };
    let outside = if keyword == "minimum" {
        number < limit
    } else {
        number > limit
    };
    if outside {
        return Err(format!("{at}: {value} is past the {keyword} {rule}"));
    }
    Ok(())
}
