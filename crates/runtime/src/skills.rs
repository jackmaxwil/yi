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

/// A skill written for one repository names a path only that repository has; the listing,
/// triggers, classifier and reads all route through this one predicate.
fn within(skill: &Skill, root: &Path) -> bool {
    (skill.frontmatter.get("within")).is_none_or(|path| root.join(path).exists())
}

/// Every skill a read can reach: the listed ones, then those a `requires:` holds back, then each
/// installed plugin's, which are reached only through a pointer and never listed.
pub fn discover(cwd: &Path, home: &Path) -> Vec<Skill> {
    let root = crate::ext::git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let (mut global, project) = scan_split(cwd, home);
    global.extend(project);
    for root in (yi_tools::plugin::PLUGINS.iter()).filter_map(|plugin| plugin.skill_root(home)) {
        global.extend(scan(&root));
    }
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for skill in global.into_iter().filter(|skill| within(skill, &root)) {
        found.entry(skill.name.clone()).or_insert(skill);
    }
    found.into_values().collect()
}

/// The user's own roots and the repository's, kept apart: a repository author writes the second
/// set's descriptions, so they render in the yard; a `requires:` unmet by an install leaves both.
pub fn discover_split(cwd: &Path, home: &Path) -> (Vec<Skill>, Vec<Skill>) {
    let (global, project) = scan_split(cwd, home);
    let root = crate::ext::git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let present = |skill: &Skill| {
        let plugin = skill.frontmatter.get("requires").is_none_or(|name| {
            yi_tools::plugin::Plugin::named(name).is_some_and(|plugin| {
                plugin.installed(home).is_some()
                    && (plugin.skills.is_empty() || plugin.skill_root(home).is_some())
            })
        });
        plugin && within(skill, &root)
    };
    (
        global.into_iter().filter(present).collect(),
        project.into_iter().filter(present).collect(),
    )
}

fn scan_split(cwd: &Path, home: &Path) -> (Vec<Skill>, Vec<Skill>) {
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

/// Each description is clipped to [`DESCRIPTION_CLIP`] chars and the whole skill is one
/// `yi://skills/<name>` read away; the block as a whole is fitted to the `skills_meta` budget.
pub fn skills_catalog(cwd: &Path, home: &Path, budget: Bytes) -> Option<Truncated> {
    catalog_text(&listing(cwd, home), budget)
}

/// Judged as the catalog and a skill read are: a global skill lists only from `~/.yi/skills`,
/// and a skill whose manifest the reader's `deny_read` covers is not named at all.
pub fn index_text(cwd: &Path, home: &Path, deny: &[PathBuf]) -> String {
    let (global, project) = discover_split(cwd, home);
    let mut skills: Vec<Skill> = (global.into_iter())
        .filter(|skill| crate::ext::project::listed(skill, home, &[]))
        .chain(project)
        .filter(|skill| !yi_tools::walled(deny, &skill.path))
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    match fitted(&skills, CATALOG_CEILING, View::Index) {
        Some(index) => index.text,
        None => "No skills found: a skill is <root>/<name>/SKILL.md under .yi, .agents, .pi or .claude/skills, in the repository or the home directory.".to_owned(),
    }
}

pub const CHILD_POINTER: &str =
    "Skills: read(\"yi://skills\") lists them; read(\"yi://skills/<name>\") loads one.";
const INHERITED_CAP: usize = 20;

#[must_use]
pub(crate) fn child_pointer(link: &crate::mailbox::ParentLink) -> String {
    let address = crate::rules::SKILL_ADDRESS;
    let inherited = inherited(link);
    let mut text = CHILD_POINTER.to_owned();
    if !inherited.is_empty() {
        let named: Vec<String> = (inherited.iter().take(INHERITED_CAP))
            .map(|name| format!("{address}{name}"))
            .collect();
        text.push_str(&format!("\nYour parent loaded: {}", named.join(", ")));
        if inherited.len() > INHERITED_CAP {
            let next = format!("read(\"{INDEX_ADDRESS}\") for every skill");
            let row = yi_tools::cut_row(
                INHERITED_CAP,
                inherited.len(),
                "parent-loaded skills named",
                "INHERITED_CAP",
                INHERITED_CAP,
                &next,
            );
            text.push_str(&format!("\n{row}"));
        }
    }
    text
}

/// A read of a file beside a skill, or of a line range, counts for the skill.
fn inherited(link: &crate::mailbox::ParentLink) -> Vec<String> {
    let Some(host) = link.host.upgrade() else {
        return Vec::new();
    };
    let Some(resolver) = host.resolver.get() else {
        return Vec::new();
    };
    let mut names = std::collections::BTreeSet::new();
    resolver.log().each_url(|url| {
        let Some(rest) = url.strip_prefix(crate::rules::SKILL_ADDRESS) else {
            return;
        };
        let path = rest.split_once('#').map_or(rest, |(path, _)| path);
        let name = path.split_once('/').map_or(path, |(name, _)| name);
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    });
    names.into_iter().collect()
}

fn listing(cwd: &Path, home: &Path) -> Vec<Skill> {
    let (mut listed, project) = discover_split(cwd, home);
    listed.extend(project);
    listed.sort_by(|a, b| a.name.cmp(&b.name));
    listed
}

#[derive(Clone, Copy)]
enum View {
    Catalog,
    Index,
}

pub const CATALOG_FLOOR: Bytes = Bytes(8_192);
pub const CATALOG_CEILING: Bytes = Bytes(32_768);
const DESCRIPTION_CLIP: usize = 60;
pub(crate) const INDEX_ADDRESS: &str = "yi://skills";

pub fn catalog_budget(context_window: u64) -> Bytes {
    let bytes = usize::try_from(context_window.saturating_mul(4) / 50).unwrap_or(usize::MAX);
    Bytes(bytes.clamp(CATALOG_FLOOR.0, CATALOG_CEILING.0))
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn render_catalog(skills: &[Skill], described: usize, view: View) -> String {
    let address = crate::rules::SKILL_ADDRESS;
    let mut body = match view {
        View::Catalog => format!(
            "<skills>\nSkills you can follow. Descriptions are clipped at {DESCRIPTION_CLIP} chars (…); read(\"{address}<name>\") returns the whole skill, and you read it before acting on one; read(\"{INDEX_ADDRESS}\") lists every skill with its whole description. `$name` in a message asks for one by name.\n"
        ),
        View::Index => format!(
            "Every skill you can follow, with its whole description; read(\"{address}<name>\") returns one, and you read it before acting on it.\n"
        ),
    };
    for skill in skills.iter().take(described) {
        body.push_str("- ");
        body.push_str(&skill.name);
        body.push_str(": ");
        match view {
            View::Catalog => body.push_str(&clip(&skill.description, DESCRIPTION_CLIP)),
            View::Index => body.push_str(&skill.description),
        }
        body.push('\n');
    }
    let rest: Vec<&str> = skills
        .iter()
        .skip(described)
        .map(|skill| skill.name.as_str())
        .collect();
    if !rest.is_empty() {
        match view {
            View::Catalog => body.push_str(&format!("+{} more: {}\n", rest.len(), rest.join(", "))),
            View::Index => {
                let next = format!("read(\"{address}<name>\") for each name below");
                let cap = format!("{INDEX_ADDRESS} cap");
                let row = yi_tools::cut_row(
                    described,
                    skills.len(),
                    "skills described",
                    &cap,
                    CATALOG_CEILING.0,
                    &next,
                );
                body.push_str(&format!("{row}\n{}\n", rest.join(", ")));
            }
        }
    }
    if let View::Catalog = view {
        body.push_str("</skills>");
    }
    body
}

pub fn catalog_text(skills: &[Skill], budget: Bytes) -> Option<Truncated> {
    fitted(skills, budget, View::Catalog)
}

fn fitted(skills: &[Skill], budget: Bytes, view: View) -> Option<Truncated> {
    if skills.is_empty() {
        return None;
    }
    let mut described = skills.len();
    loop {
        let body = render_catalog(skills, described, view);
        if body.len() <= budget.0 {
            return Some(Truncated {
                text: body,
                truncated: described < skills.len(),
            });
        }
        if described == 0 {
            let next = format!(
                "read(\"{}<name>\") for one skill",
                crate::rules::SKILL_ADDRESS
            );
            return Some(fit(&body, budget, &next));
        }
        described = described.saturating_sub(1);
    }
}

fn scan(root: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    collect(root, 2, &mut skills);
    skills
}

/// A root holds skill directories, and the bundled sets (§7.8) group theirs
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
pub fn frontmatter(source: &str) -> BTreeMap<String, String> {
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
