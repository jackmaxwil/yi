use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use yi_tools::ToolKind;

/// Design B1 overlay, plan §3.4: a reduction of the child's capability set,
/// never an extension. Expand-only enforcement lives here — an implementer
/// child cannot edit the standard it is measured against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wall {
    pub deny_write: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
}

fn parse_paths(value: Option<&Value>, cwd: &Path, key: &str) -> Result<Vec<PathBuf>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("rlm.run {key} must be a list of paths"))?;
    entries
        .iter()
        .map(|entry| {
            let text = entry
                .as_str()
                .ok_or_else(|| format!("rlm.run {key} entries must be strings, got {entry}"))?;
            let path = PathBuf::from(text);
            Ok(if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            })
        })
        .collect()
}

impl Wall {
    pub fn from_kwargs(kwargs: &Map<String, Value>, cwd: &Path) -> Result<Self, String> {
        Ok(Self {
            deny_write: parse_paths(kwargs.get("deny_write"), cwd, "deny_write")?,
            deny_read: parse_paths(kwargs.get("deny_read"), cwd, "deny_read")?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.deny_write.is_empty() && self.deny_read.is_empty()
    }

    /// Denies before the call runs, naming the path as evidence. Not a sandbox:
    /// a command naming no denied path runs, stopping an honest agent, not an evasive one.
    pub fn check(
        &self,
        tool_name: &str,
        kind: ToolKind,
        args: &Map<String, Value>,
        cwd: &Path,
    ) -> Option<String> {
        let denied: Vec<&PathBuf> = if matches!(kind, ToolKind::Read) {
            self.deny_read.iter().collect()
        } else {
            self.deny_write
                .iter()
                .chain(self.deny_read.iter())
                .collect()
        };
        if denied.is_empty() {
            return None;
        }
        for target in crate::permission::extract_targets(tool_name, args, cwd) {
            let normalized = yi_permission::lexical_normalize(&target);
            if let Some(hit) = denied
                .iter()
                .find(|denied| normalized.starts_with(yi_permission::lexical_normalize(denied)))
            {
                return Some(refusal(tool_name, &hit.display().to_string()));
            }
        }
        let command = args.get("command").and_then(Value::as_str)?;
        denied
            .iter()
            .find(|denied| command.contains(&denied.to_string_lossy().into_owned()))
            .map(|hit| refusal(tool_name, &hit.display().to_string()))
    }
}

fn refusal(tool_name: &str, path: &str) -> String {
    format!(
        "Denied by the reviewer wall: {tool_name} targets {path}, which this agent may not touch. \
         The standard is fixed for the run — report the mismatch instead of changing it."
    )
}
