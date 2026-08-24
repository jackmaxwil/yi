use serde_json::Value;
use yi_types::model::Model;

pub(crate) fn compat_bool(model: &Model, key: &str, default: bool) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

pub(crate) fn compat_str<'a>(model: &'a Model, key: &str) -> Option<&'a str> {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get(key))
        .and_then(Value::as_str)
}
