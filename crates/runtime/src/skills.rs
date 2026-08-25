use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use yi_context::{Bytes, Truncated, fit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Design §5 / §14.1: project root, then global root. A skill in the project
/// shadows a global one of the same name, and the bundled set is installed
/// into the global root rather than compiled into the binary.
pub fn roots(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![home.join(".yi/skills"), cwd.join(".yi/skills")]
}

pub fn discover(cwd: &Path, home: &Path) -> Vec<Skill> {
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for root in roots(cwd, home) {
        for skill in scan(&root) {
            found.insert(skill.name.clone(), skill);
        }
    }
    found.into_values().collect()
}

/// P16: `description` is the trigger surface, so the catalog carries it in
/// full and the whole block is fitted to the `skills_meta` budget.
pub fn skills_catalog(cwd: &Path, home: &Path, budget: Bytes) -> Option<Truncated> {
    let skills = discover(cwd, home);
    if skills.is_empty() {
        return None;
    }
    let mut body = String::from(
        "<skills>\nSkills you can follow. Read the file with `read` before acting on one.\n",
    );
    for skill in &skills {
        body.push_str("- ");
        body.push_str(&skill.name);
        body.push_str(": ");
        body.push_str(&skill.description);
        body.push_str(" (");
        body.push_str(&skill.path.display().to_string());
        body.push_str(")\n");
    }
    body.push_str("</skills>");
    Some(fit(&body, budget))
}

fn scan(root: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    collect(root, 2, &mut skills);
    skills
}

/// A root holds skill directories, and the bundled sets (§14.1) group theirs
/// one level deeper, so both layouts are walked.
fn collect(dir: &Path, depth: u8, skills: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("SKILL.md");
        if manifest.is_file() {
            if let Some(skill) = read_skill(&path, &manifest) {
                skills.push(skill);
            }
            continue;
        }
        if depth > 1 {
            collect(&path, depth.saturating_sub(1), skills);
        }
    }
}

fn read_skill(dir: &Path, manifest: &Path) -> Option<Skill> {
    let source = std::fs::read_to_string(manifest).ok()?;
    let fields = frontmatter(&source);
    let name = fields.get("name").cloned().or_else(|| {
        dir.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })?;
    if name.is_empty() {
        return None;
    }
    Some(Skill {
        description: fields.get("description").cloned().unwrap_or_default(),
        name,
        path: manifest.to_path_buf(),
    })
}

/// The `key: value` subset of YAML that skill frontmatter actually uses;
/// anything else in the block is ignored rather than guessed at.
fn frontmatter(source: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut lines = source.lines();
    if lines.next().map(str::trim) != Some("---") {
        return fields;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            continue;
        }
        fields.insert(key.trim().to_owned(), value.to_owned());
    }
    fields
}
