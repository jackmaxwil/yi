//! The internal cells that read or dill one kernel variable (D164) and the line they print.

use std::path::Path;

use serde_json::Value;

use crate::kernel::{VARIABLE_MARKER, VARIABLE_MAX_CHARS, VariableName};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VariableReply {
    Missing,
    Value { text: String, chars: usize },
    Unreadable { python: String },
}

pub(crate) fn render_value(text: String, chars: usize) -> String {
    let shown = text.chars().count();
    if chars <= shown {
        return text;
    }
    format!("{text}\n[... truncated: {shown} of {chars} chars ...]")
}

pub(crate) fn py_literal(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

/// Invariant: a page never widens the cell's reply past [`VARIABLE_MAX_CHARS`] (D213).
pub(crate) fn read_variable_code(name: &VariableName, page: Option<crate::fetch::Page>) -> String {
    let start = page.map_or(0, |page| page.offset);
    let limit = page.map_or(VARIABLE_MAX_CHARS, |page| {
        page.limit.min(VARIABLE_MAX_CHARS)
    });
    format!(
        r#"def _yi_read_variable():
    import builtins as _b, json
    ip = None
    try:
        ip = get_ipython()  # noqa: F821 (injected by IPython)
    except _b.Exception:
        ip = None
    ns = ip.user_ns if ip is not None else _b.globals()
    name = {name}
    if name not in ns:
        _b.print({marker} + json.dumps({{"found": False}}))
        return
    try:
        text = _b.repr(ns[name])
        payload = json.dumps({{"found": True, "chars": _b.len(text), "text": text[{start}:{end}]}})
    except _b.BaseException as exc:
        payload = json.dumps({{"found": True, "error": _b.repr(exc)}})
    _b.print({marker} + payload)


try:
    _yi_read_variable()
finally:
    del _yi_read_variable"#,
        name = py_literal(name.as_str()),
        marker = py_literal(VARIABLE_MARKER),
        end = start.saturating_add(limit),
    )
}

/// Dills (pickle when dill is absent) one variable to `path`; the reply's `text` is the path
/// and `chars` its size, so [`parse_variable_reply`] reads both cells.
pub(crate) fn dump_variable_code(name: &VariableName, path: &Path) -> String {
    format!(
        r#"def _yi_dump_variable():
    import builtins as _b, json, os
    ip = None
    try:
        ip = get_ipython()  # noqa: F821 (injected by IPython)
    except _b.Exception:
        ip = None
    ns = ip.user_ns if ip is not None else _b.globals()
    name = {name}
    path = {path}
    if name not in ns:
        _b.print({marker} + json.dumps({{"found": False}}))
        return
    try:
        try:
            import dill as _ser
        except _b.ImportError:
            import pickle as _ser
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = path + ".tmp-" + _b.str(os.getpid())
        try:
            try:
                os.remove(tmp)
            except _b.FileNotFoundError:
                pass
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
            with os.fdopen(os.open(tmp, flags, 0o600), "wb") as handle:
                _ser.dump(ns[name], handle)
            os.replace(tmp, path)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
        payload = json.dumps({{"found": True, "chars": os.path.getsize(path), "text": path}})
    except _b.BaseException as exc:
        payload = json.dumps({{"found": True, "error": _b.repr(exc)}})
    _b.print({marker} + payload)


try:
    _yi_dump_variable()
finally:
    del _yi_dump_variable"#,
        name = py_literal(name.as_str()),
        path = py_literal(&path.to_string_lossy()),
        marker = py_literal(VARIABLE_MARKER),
    )
}

pub(crate) fn parse_variable_reply(stdout: &str) -> Option<VariableReply> {
    let index = stdout.rfind(VARIABLE_MARKER)?;
    let rest = &stdout[index.saturating_add(VARIABLE_MARKER.len())..];
    let value: Value = serde_json::from_str(rest.lines().next()?.trim()).ok()?;
    if !value.get("found")?.as_bool()? {
        return Some(VariableReply::Missing);
    }
    if let Some(python) = value.get("error").and_then(Value::as_str) {
        return Some(VariableReply::Unreadable {
            python: python.to_owned(),
        });
    }
    Some(VariableReply::Value {
        text: value.get("text")?.as_str()?.to_owned(),
        chars: usize::try_from(value.get("chars")?.as_u64()?).ok()?,
    })
}
