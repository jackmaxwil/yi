use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Effect, Event, EventMask, Extension, Rank, Slot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub name: String,
    pub fragment: String,
    pub project_markers: Vec<String>,
    pub write_extensions: Vec<String>,
}

impl Pack {
    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let value: Value = serde_json::from_str(&raw).ok()?;
        let dir = path.parent()?;
        let name = value.get("name").and_then(Value::as_str)?.to_owned();
        let fragment = match value.get("fragment").and_then(Value::as_str) {
            Some(file) => std::fs::read_to_string(dir.join(file)).ok()?,
            None => value.get("text").and_then(Value::as_str)?.to_owned(),
        };
        Some(Self {
            name,
            fragment,
            project_markers: strings(&value, "project_markers"),
            write_extensions: strings(&value, "write_extensions"),
        })
    }

    pub fn load_dir(dir: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        paths.iter().filter_map(|path| Self::load(path)).collect()
    }
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub struct PackExtension {
    pack: Pack,
    cwd: PathBuf,
    attached: bool,
}

impl PackExtension {
    pub fn new(pack: Pack, cwd: PathBuf) -> Self {
        Self {
            pack,
            cwd,
            attached: false,
        }
    }

    fn attach(&mut self, out: &mut Vec<Effect>, remind: bool) {
        if self.attached {
            return;
        }
        self.attached = true;
        out.push(Effect::AttachFragment {
            slot: Slot::new(Rank::Lang, &self.pack.name),
            text: self.pack.fragment.clone(),
        });
        if remind {
            out.push(Effect::Remind {
                text: format!(
                    "{} applies to this file; its rules are now loaded.",
                    self.pack.name
                ),
            });
        }
    }

    /// No triggers at all means the pack is unconditional: a house-style
    /// fragment applies to the repository that ships it, not to a file type.
    fn marker_present(&self) -> bool {
        if self.pack.project_markers.is_empty() && self.pack.write_extensions.is_empty() {
            return true;
        }
        self.pack.project_markers.iter().any(|marker| {
            if self.cwd.join(marker).exists() {
                return true;
            }
            let Ok(entries) = std::fs::read_dir(&self.cwd) else {
                return false;
            };
            entries
                .flatten()
                .any(|entry| entry.path().join(marker).exists())
        })
    }

    fn writes_a_covered_file(&self, name: &str, target: Option<&Path>) -> bool {
        if name != "write" && name != "edit" {
            return false;
        }
        let Some(extension) = target.and_then(Path::extension) else {
            return false;
        };
        self.pack
            .write_extensions
            .iter()
            .any(|covered| extension == covered.as_str())
    }
}

impl Extension for PackExtension {
    fn name(&self) -> &'static str {
        "pack"
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START.with(EventMask::TOOL_CALL)
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        match event {
            Event::SessionStart { .. } => {
                if self.marker_present() {
                    self.attach(out, false);
                }
            }
            Event::ToolCall { name, target, .. } => {
                if self.writes_a_covered_file(name, target.as_deref()) {
                    self.attach(out, true);
                }
            }
            _ => {}
        }
    }
}
