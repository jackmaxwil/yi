use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use yi_types::harness::{HarnessEntry, HarnessKind};

use crate::budget::{Bytes, fit};

const KINDS: [HarnessKind; 3] = [
    HarnessKind::Prompt,
    HarnessKind::Memory,
    HarnessKind::Subagent,
];

fn kind_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Prompt => "prompt",
        HarnessKind::Memory => "memory",
        HarnessKind::Subagent => "subagent",
    }
}

/// Design P10: the harness ledger, read-side. The file is prime-agent's
/// `harness.py` state shape (`entries.{kind}.{id}`); the kernel (phase 4)
/// writes it from Python while Yi re-reads on mtime change. Corrupt or
/// missing files load as empty — the ledger must never block a turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HarnessState {
    pub entries: BTreeMap<HarnessKind, Vec<HarnessEntry>>,
    file_path: Option<PathBuf>,
    loaded_mtime: Option<SystemTime>,
}

fn disk_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

fn parse_entries(data: &Value) -> BTreeMap<HarnessKind, Vec<HarnessEntry>> {
    let mut entries: BTreeMap<HarnessKind, Vec<HarnessEntry>> = BTreeMap::new();
    let Some(raw_entries) = data.get("entries").and_then(Value::as_object) else {
        return entries;
    };
    for kind in KINDS {
        let Some(kind_entries) = raw_entries.get(kind_key(kind)).and_then(Value::as_object) else {
            continue;
        };
        let mut parsed: Vec<HarnessEntry> = Vec::new();
        for (id, raw) in kind_entries {
            let mut raw = raw.clone();
            if let Some(object) = raw.as_object_mut() {
                object.insert("id".to_owned(), Value::String(id.clone()));
                object.insert("kind".to_owned(), Value::String(kind_key(kind).to_owned()));
            }
            if let Ok(entry) = serde_json::from_value::<HarnessEntry>(raw) {
                parsed.push(entry);
            }
        }
        parsed.sort_by(|a, b| (&a.path, &a.title, &a.id).cmp(&(&b.path, &b.title, &b.id)));
        entries.insert(kind, parsed);
    }
    entries
}

impl HarnessState {
    pub fn load(path: &Path) -> Self {
        let mtime = disk_mtime(path);
        let data = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null);
        Self {
            entries: parse_entries(&data),
            file_path: Some(path.to_path_buf()),
            loaded_mtime: mtime,
        }
    }

    /// Reloads only when another process rewrote the file since the last
    /// load — the mtime guard from prime `harness.py:_sync_from_disk`.
    pub fn sync_from_disk(&mut self) {
        let Some(path) = self.file_path.clone() else {
            return;
        };
        if disk_mtime(&path) != self.loaded_mtime {
            *self = Self::load(&path);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.values().all(Vec::is_empty)
    }

    fn compact_text(text: &str, max_length: usize) -> String {
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.len() <= max_length {
            return normalized;
        }
        let mut end = max_length.saturating_sub(3);
        while end > 0 && !normalized.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        format!("{}...", &normalized[..end])
    }

    /// Compact routing-hint rendering for the system prompt, held to the
    /// ledger byte budget (P16).
    pub fn format_for_prompt(&self, budget: Bytes) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut lines = vec![
            "# Harness Ledger".to_owned(),
            String::new(),
            "The entries below are compact summaries recorded across sessions. Use them as routing and context hints; they survive compaction.".to_owned(),
        ];
        for kind in KINDS {
            let Some(entries) = self.entries.get(&kind).filter(|list| !list.is_empty()) else {
                continue;
            };
            lines.push(String::new());
            lines.push(format!("## {}", kind_key(kind)));
            for entry in entries {
                let scope = match entry.scope {
                    yi_types::harness::HarnessScope::Local => "local",
                    yi_types::harness::HarnessScope::Global => "global",
                };
                lines.push(format!(
                    "- [{scope}:{}] {}: {}",
                    entry.id,
                    entry.title,
                    Self::compact_text(&entry.content, 200)
                ));
            }
        }
        Some(fit(&lines.join("\n"), budget).text)
    }
}
