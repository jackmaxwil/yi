//! The internal cells that read or dill one kernel variable (D164) and the line they print.

use std::path::Path;

use serde_json::Value;

pub(crate) const VARIABLE_MARKER: &str = "__yi_kernel_var__";
pub(crate) const VARIABLE_MAX_CHARS: usize = 8_192;
pub(crate) const VARIABLE_NAME_MAX_BYTES: usize = 128;

/// Invariant: an ASCII Python identifier, never a dotted path: the name is
/// interpolated into a cell, and attribute access runs arbitrary code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableName(String);

impl VariableName {
    pub fn parse(raw: &str) -> Result<Self, VariableReadError> {
        let head_ok = raw
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
        let body_ok = raw
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '_');
        if head_ok && body_ok && raw.len() <= VARIABLE_NAME_MAX_BYTES {
            return Ok(Self(raw.to_owned()));
        }
        Err(VariableReadError::NotAnIdentifier {
            name: raw.to_owned(),
            max: VARIABLE_NAME_MAX_BYTES,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VariableName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VariableReadError {
    #[error("{name:?} is not an ASCII Python identifier of 1 to {max} bytes")]
    NotAnIdentifier { name: String, max: usize },
    #[error("no IPython kernel is running for this agent")]
    NotRunning,
    #[error("the kernel could not be read: {detail}")]
    Cell { detail: String },
    #[error(
        "the agent is running a cell; nothing was read within {} s",
        yi_kernel::KERNEL_STATE_LISTING_TIMEOUT_MS / 1000
    )]
    Busy,
    #[error("repr({name}) raised {python}")]
    Unreadable { name: VariableName, python: String },
}

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
