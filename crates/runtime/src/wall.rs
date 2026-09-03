use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use yi_tools::ToolKind;
use yi_types::url::{Scheme, Url};

/// Design B1 overlay, plan §3.4: a reduction of the child's capability set, never an
/// extension, so an implementer child cannot edit the standard it is measured against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wall {
    pub deny_write: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
    pub deny_url: Vec<String>,
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

/// A deny entry is matched at an address boundary, so a bare scheme
/// (`kernel://`) walls the whole scheme and a longer prefix walls a path.
fn parse_prefixes(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| "rlm.run deny_url must be a list of URL prefixes".to_owned())?;
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("rlm.run deny_url entries must be strings, got {entry}"))
        })
        .collect()
}

impl Wall {
    pub fn from_kwargs(kwargs: &Map<String, Value>, cwd: &Path) -> Result<Self, String> {
        Ok(Self {
            deny_write: parse_paths(kwargs.get("deny_write"), cwd, "deny_write")?,
            deny_read: parse_paths(kwargs.get("deny_read"), cwd, "deny_read")?,
            deny_url: parse_prefixes(kwargs.get("deny_url"))?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.deny_write.is_empty() && self.deny_read.is_empty() && self.deny_url.is_empty()
    }

    /// Invariant: a fetch is read-only, so only [`Wall::deny_read`] maps into
    /// URL space — a read-walled path is also a walled `local://` URL.
    pub fn check_url(&self, url: &Url, workspace: &Path) -> Option<String> {
        let rendered = url.to_string();
        if let Some(hit) = self.deny_url.iter().find(|prefix| walls(prefix, &rendered)) {
            return Some(refusal("fetch", hit));
        }
        let raw = match url.scheme() {
            Scheme::Local => Path::new(url.path()),
            Scheme::Checkpoint => {
                Path::new(url.path().split_once('/').map_or("", |(_, path)| path))
            }
            Scheme::Kernel
            | Scheme::Plan
            | Scheme::Agent
            | Scheme::History
            | Scheme::Mcp
            | Scheme::User
            | Scheme::External(_) => return None,
        };
        let target = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            workspace.join(raw)
        };
        self.check_read_path(&target)
    }

    /// Invariant: the wall covers the path a read lands on, so a link out of one tree into a
    /// denied one is refused on the target; the root canonicalizes only after a lexical miss.
    pub fn check_read_path(&self, path: &Path) -> Option<String> {
        let normalized = yi_permission::lexical_normalize(path);
        self.deny_read
            .iter()
            .find(|denied| {
                normalized.starts_with(yi_permission::lexical_normalize(denied))
                    || std::fs::canonicalize(denied).is_ok_and(|real| normalized.starts_with(real))
            })
            .map(|hit| refusal("fetch", &hit.display().to_string()))
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

/// Incident: a raw prefix walled every address beginning with it, so `plan://secret` refused
/// `plan://secretary`. The match ends at the entry, a path separator, or a fragment.
fn walls(prefix: &str, rendered: &str) -> bool {
    let Some(rest) = rendered.strip_prefix(prefix) else {
        return false;
    };
    rest.is_empty() || prefix.ends_with('/') || rest.starts_with('/') || rest.starts_with('#')
}

fn refusal(tool_name: &str, path: &str) -> String {
    format!(
        "Denied by the reviewer wall: {tool_name} targets {path}, which this agent may not touch. \
         The standard is fixed for the run — report the mismatch instead of changing it."
    )
}
