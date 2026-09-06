use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use yi_context::{Bytes, Truncated, fit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub frontmatter: BTreeMap<String, String>,
}

/// Project roots, then global, own format before the compatibility conventions. First root
/// wins a name, so a project skill shadows a global one and `.yi` shadows the rest.
pub fn roots(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    crate::ext::resource_roots(cwd, home, "skills")
}

pub fn discover(cwd: &Path, home: &Path) -> Vec<Skill> {
    let (mut global, project) = discover_split(cwd, home);
    global.extend(project);
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for skill in global {
        found.entry(skill.name.clone()).or_insert(skill);
    }
    found.into_values().collect()
}

/// The user's own roots and the repository's, kept apart: a repository author writes the
/// second set's descriptions, so those render in the yard, not the trusted prefix.
pub fn discover_split(cwd: &Path, home: &Path) -> (Vec<Skill>, Vec<Skill>) {
    let mut global: BTreeMap<String, Skill> = BTreeMap::new();
    let mut project: BTreeMap<String, Skill> = BTreeMap::new();
    for root in roots(cwd, home) {
        let target = if crate::ext::is_project_root(&root, cwd, home) {
            &mut project
        } else {
            &mut global
        };
        for skill in scan(&root) {
            target.entry(skill.name.clone()).or_insert(skill);
        }
    }
    for name in project.keys() {
        global.remove(name);
    }
    (
        global.into_values().collect(),
        project.into_values().collect(),
    )
}

/// `description` is the trigger surface, so it is carried in full and the block
/// as a whole is fitted to the `skills_meta` budget.
pub fn skills_catalog(cwd: &Path, home: &Path, budget: Bytes) -> Option<Truncated> {
    catalog_text(&discover(cwd, home), budget)
}

pub fn catalog_text(skills: &[Skill], budget: Bytes) -> Option<Truncated> {
    if skills.is_empty() {
        return None;
    }
    let mut body = String::from(
        "<skills>\nSkills you can follow. Read the file with `read` before acting on one.\n",
    );
    for skill in skills {
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
        frontmatter: fields,
    })
}

/// The `key: value` subset of YAML that skill frontmatter actually uses, plus
/// `>` and `|` blocks; anything else in the block is ignored rather than guessed at.
pub(crate) fn frontmatter(source: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut lines = source.lines().peekable();
    if lines.next().map(str::trim) != Some("---") {
        return fields;
    }
    while let Some(line) = lines.next() {
        if line.trim() == "---" {
            break;
        }
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        let value = match value.trim_end_matches('-') {
            fold @ (">" | "|") => {
                let mut block = Vec::new();
                while let Some(next) = lines.peek() {
                    if !next.starts_with(char::is_whitespace) || next.trim() == "---" {
                        break;
                    }
                    block.push(next.trim().to_owned());
                    lines.next();
                }
                block.join(if fold == ">" { " " } else { "\n" })
            }
            _ => value.to_owned(),
        };
        if value.is_empty() {
            continue;
        }
        fields.insert(key.trim().to_owned(), value);
    }
    fields
}
