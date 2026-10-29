//! What a turn's reasoning must carry back to the provider: OpenRouter's `reasoning_details` and
//! Google's `thought_signature`, both opaque and both refused by the next tool turn when dropped.

use serde_json::{Map, Value, json};
use yi_types::message::Content;

pub(crate) const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];
const CARRY_FIELD: &str = "field";
const CARRY_DETAILS: &str = "details";

fn reasoning_carry(signature: Option<&str>) -> Option<(String, Option<Value>)> {
    let signature = signature?;
    if let Ok(carry) = serde_json::from_str::<Value>(signature)
        && let Some(field) = carry.get(CARRY_FIELD).and_then(Value::as_str)
    {
        let details = carry.get(CARRY_DETAILS).filter(|value| value.is_array());
        return Some((field.to_owned(), details.cloned()));
    }
    REASONING_FIELDS
        .contains(&signature)
        .then(|| (signature.to_owned(), None))
}

fn merge_detail(kept: &mut Value, entry: &Value) {
    let (Some(kept), Some(patch)) = (kept.as_object_mut(), entry.as_object()) else {
        return;
    };
    for (key, value) in patch {
        match (kept.get_mut(key), value.as_str()) {
            (Some(Value::String(kept_text)), Some(text)) if key == "text" => {
                kept_text.push_str(text);
            }
            (Some(slot), _) => *slot = value.clone(),
            (None, _) => {
                kept.insert(key.clone(), value.clone());
            }
        }
    }
}

/// One turn's details as they arrive: per `index`, text appended and the rest replaced, so what
/// the block seals is the array the provider sent.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReasoningCarry {
    details: Vec<Value>,
}

impl ReasoningCarry {
    pub(crate) fn absorb(&mut self, delta: &Value) {
        let Some(entries) = delta.get("reasoning_details").and_then(Value::as_array) else {
            return;
        };
        for entry in entries {
            let index = entry.get("index").and_then(Value::as_u64);
            let kept = index.and_then(|index| {
                self.details
                    .iter_mut()
                    .find(|kept| kept.get("index").and_then(Value::as_u64) == Some(index))
            });
            match kept {
                Some(kept) => merge_detail(kept, entry),
                None => self.details.push(entry.clone()),
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.details.is_empty()
    }

    /// The block's new signature, or `None` when no details arrived; the plain delta field name
    /// stays as it was, so a turn with no details still sends none.
    pub(crate) fn seal(&mut self, signature: Option<&str>) -> Option<String> {
        if self.details.is_empty() {
            return None;
        }
        let field = reasoning_carry(signature)
            .map(|(field, _)| field)
            .unwrap_or_else(|| REASONING_FIELDS[1].to_owned());
        let details = Value::Array(std::mem::take(&mut self.details));
        Some(json!({CARRY_FIELD: field, CARRY_DETAILS: details}).to_string())
    }
}

/// Renders the reasoning a chat assistant message carries: the prose under the field the reply
/// arrived in, and the details the turn sealed.
pub(crate) fn apply(message: &mut Value, content: &[Content]) {
    let carry = content.iter().find_map(|block| match block {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            let (field, details) = reasoning_carry(thinking_signature.as_deref())?;
            (!thinking.trim().is_empty() || details.is_some()).then_some((thinking, field, details))
        }
        _ => None,
    });
    let Some((thinking, field, details)) = carry else {
        return;
    };
    if !thinking.trim().is_empty() {
        message[field.as_str()] = Value::String(thinking.clone());
    }
    if let Some(details) = details {
        message["reasoning_details"] = details;
    }
}

/// A tool call as the chat wire takes it, with the signature Gemini's function call came with.
pub(crate) fn tool_call_param(
    id: &str,
    name: &str,
    arguments: &Map<String, Value>,
    thought_signature: Option<&str>,
) -> Value {
    let mut call = json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": Value::Object(arguments.clone()).to_string(),
        },
    });
    if let Some(signature) = thought_signature {
        call["extra_content"] = json!({"google": {"thought_signature": signature}});
    }
    call
}

pub(crate) fn thought_signature_of(tool_call: &Value) -> Option<String> {
    tool_call
        .get("extra_content")
        .and_then(|extra| extra.get("google"))
        .and_then(|google| google.get("thought_signature"))
        .and_then(Value::as_str)
        .or_else(|| tool_call.get("thought_signature").and_then(Value::as_str))
        .map(str::to_owned)
}
