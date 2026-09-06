use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{Effect, Event, EventMask, Extension, Rank, Slot, Trust};

const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

const RESOURCE_DIRS: [&str; 4] = [".yi", ".agents", ".pi", ".claude"];

pub fn resource_roots(cwd: &Path, home: &Path, kind: &str) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(RESOURCE_DIRS.len().saturating_mul(2));
    for base in [cwd, home] {
        for dir in RESOURCE_DIRS {
            roots.push(base.join(dir).join(kind));
        }
    }
    roots
}

pub fn is_project_root(root: &Path, cwd: &Path, home: &Path) -> bool {
    root.starts_with(cwd) && cwd != home
}

/// Trust-on-first-use, pinned to the granted content: an edit after the grant reads as
/// untrusted until granted again, so a `git pull` cannot launder authority.
pub struct TrustGate {
    path: PathBuf,
}

impl TrustGate {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join(".yi/trust.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn table(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    pub fn trust_of(&self, root: &Path, source: &str, content: &str) -> Trust {
        let table = self.table();
        let granted = table
            .get(&root.display().to_string())
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(source))
            .and_then(Value::as_str);
        match granted {
            Some(hash) if hash == content_hash(content) => Trust::Granted,
            _ => Trust::Untrusted,
        }
    }

    pub fn grant(&self, root: &Path, sources: &[(String, String)]) -> Result<(), String> {
        let mut table = self.table();
        let mut entry = Map::new();
        for (source, content) in sources {
            entry.insert(source.clone(), Value::String(content_hash(content)));
        }
        table.insert(root.display().to_string(), Value::Object(entry));
        write_table(&self.path, &table)
    }

    pub fn revoke(&self, root: &Path) -> Result<(), String> {
        let mut table = self.table();
        table.remove(&root.display().to_string());
        write_table(&self.path, &table)
    }

    pub fn grants(&self) -> Vec<(String, usize)> {
        self.table()
            .iter()
            .map(|(root, entries)| {
                (
                    root.clone(),
                    entries.as_object().map_or(0, serde_json::Map::len),
                )
            })
            .collect()
    }
}

fn write_table(path: &Path, table: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(&Value::Object(table.clone()))
        .map_err(|error| error.to_string())?;
    std::fs::write(path, body).map_err(|error| error.to_string())
}

pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn git_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

pub struct ProjectResources {
    cwd: PathBuf,
    home: PathBuf,
    gate: TrustGate,
    budget: yi_context::Bytes,
    catalog: yi_context::Bytes,
}

impl ProjectResources {
    pub fn new(cwd: PathBuf, home: PathBuf) -> Self {
        let gate = TrustGate::new(&home);
        Self {
            cwd,
            home,
            gate,
            budget: yi_context::SourceBudgets::default().project_instructions,
            catalog: yi_context::SourceBudgets::default().skills_meta,
        }
    }

    #[must_use]
    pub fn with_context_window(mut self, context_window: u64) -> Self {
        self.catalog = crate::skills::catalog_budget(context_window);
        self
    }

    pub fn instruction_files(cwd: &Path) -> Vec<PathBuf> {
        let mut roots = vec![cwd.to_path_buf()];
        if let Some(root) = git_root(cwd)
            && root != cwd
        {
            roots.push(root);
        }
        let mut found = Vec::new();
        for root in roots {
            for name in INSTRUCTION_FILES {
                let path = root.join(name);
                if path.is_file() {
                    found.push(path);
                }
            }
        }
        found
    }

    fn instructions(&self, out: &mut Vec<Effect>) {
        let root = git_root(&self.cwd).unwrap_or_else(|| self.cwd.clone());
        let mut seen = Vec::new();
        for path in Self::instruction_files(&self.cwd) {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let hash = content_hash(&content);
            if seen.contains(&hash) {
                continue;
            }
            seen.push(hash);
            let source = display_source(&path, &root);
            let trust = self.gate.trust_of(&root, &source, &content);
            out.push(Effect::AttachExternal {
                source,
                trust,
                text: yi_context::fit(&content, self.budget).text,
            });
        }
    }

    fn catalogs(&self, out: &mut Vec<Effect>) {
        let (global, project) = crate::skills::discover_split(&self.cwd, &self.home);
        if let Some(catalog) = crate::skills::catalog_text(&global, self.catalog) {
            out.push(Effect::AttachFragment {
                slot: Slot::new(Rank::Catalog, "skills"),
                text: catalog.text,
            });
        }
        if project.is_empty() {
            return;
        }
        let root = git_root(&self.cwd).unwrap_or_else(|| self.cwd.clone());
        let Some(catalog) = crate::skills::catalog_text(&project, self.catalog) else {
            return;
        };
        let trust = self.gate.trust_of(&root, "skills", &catalog.text);
        out.push(Effect::AttachExternal {
            source: "project skills".to_owned(),
            trust,
            text: catalog.text,
        });
    }
}

pub fn contributions(cwd: &Path, home: &Path) -> Vec<(String, String)> {
    let root = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let mut found = Vec::new();
    for path in ProjectResources::instruction_files(cwd) {
        if let Ok(content) = std::fs::read_to_string(&path) {
            found.push((display_source(&path, &root), content));
        }
    }
    let (_, project) = crate::skills::discover_split(cwd, home);
    if let Some(catalog) =
        crate::skills::catalog_text(&project, yi_context::SourceBudgets::default().skills_meta)
    {
        found.push(("skills".to_owned(), catalog.text));
    }
    for (name, body) in pack_files(cwd) {
        found.push((format!("extensions/{name}"), body));
    }
    found
}

pub fn pack_files(root: &Path) -> Vec<(String, String)> {
    let dir = root.join(".yi/extensions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let body = std::fs::read_to_string(path).ok()?;
            Some((name, body))
        })
        .collect()
}

fn display_source(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

impl Extension for ProjectResources {
    fn name(&self) -> &'static str {
        "project-resources"
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        if !matches!(event, Event::SessionStart { .. }) {
            return;
        }
        self.instructions(out);
        self.catalogs(out);
    }
}
