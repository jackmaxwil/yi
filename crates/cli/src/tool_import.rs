use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};

use crate::Args;

const USAGE: &str = "usage: yi tool import just [recipe ...]";
const MARKER: &str = "# yi: ";
const MARKED: &str = "# yi: {";
const SKIP: &str = "yi: skip";
const READ: &str = "yi: read";
const READ_NAMES: [&str; 8] = ["cat", "diff", "info", "list", "log", "ls", "show", "status"];
const WIRED: [&str; 7] = [
    "annotate",
    "answer",
    "ask_user",
    "ipython",
    "plan",
    "todo",
    "web_search",
];
const ARGV: &str = r#"import json, os, sys
spec = json.loads(sys.argv[1])
call = json.load(sys.stdin)
if not isinstance(call, dict):
    sys.exit("yi: the just wrapper takes a JSON object on stdin")
argv = ["just", "--justfile", spec["justfile"], spec["recipe"]]
for name, variadic in spec["parameters"]:
    value = call.get(name)
    if value is None:
        continue
    if variadic:
        argv.extend(str(item) for item in (value if isinstance(value, list) else [value]))
    else:
        argv.append(str(value))
os.execvp(argv[0], argv)"#;

pub(crate) struct Param {
    name: String,
    variadic: bool,
    required: bool,
    default: Option<String>,
}

pub(crate) struct Recipe {
    name: String,
    doc: Option<String>,
    params: Vec<Param>,
}

pub(crate) struct Dump {
    recipes: Vec<Recipe>,
    skipped: Vec<String>,
    module_recipes: usize,
}

#[derive(Default)]
pub(crate) struct Report {
    added: Vec<String>,
    updated: Vec<String>,
    unchanged: Vec<String>,
    removed: Vec<String>,
    skipped: Vec<String>,
    module_recipes: usize,
}

pub(crate) fn run(args: &Args) -> i32 {
    let words: Vec<&str> = args.prompt.split_whitespace().collect();
    let picked = match words.as_slice() {
        ["import", "just", rest @ ..] => rest,
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match import(&cwd(args), picked) {
        Ok(code) => code,
        Err(reason) => {
            eprintln!("error: {reason}");
            2
        }
    }
}

fn cwd(args: &Args) -> PathBuf {
    args.cwd
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[expect(
    clippy::disallowed_methods,
    reason = "just is the only reader of its own recipes, and its dump is exact"
)]
fn import(cwd: &Path, picked: &[&str]) -> Result<i32, String> {
    let dump = Command::new("just")
        .args(["--dump", "--dump-format", "json"])
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("yi tool import just needs `just` on PATH: {error}"))?;
    if !dump.status.success() {
        return Err(format!(
            "just --dump failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&dump.stderr).trim()
        ));
    }
    let text = String::from_utf8(dump.stdout)
        .map_err(|error| format!("just --dump printed no UTF-8: {error}"))?;
    let justfile = justfile(&text)?;
    let dir = crate::update::absolute_home()?.join(".yi/tools");
    let report = sync(&dir, &justfile, &text, picked)?;
    print(&dir, &justfile, &report);
    Ok(0)
}

fn justfile(dump: &str) -> Result<PathBuf, String> {
    parse(dump)?
        .get("source")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "just --dump named no source justfile".to_owned())
}

fn parse(dump: &str) -> Result<Value, String> {
    serde_json::from_str(dump)
        .map_err(|error| format!("just --dump printed something else: {error}"))
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn recipes(dump: &str) -> Result<Dump, String> {
    let value = parse(dump)?;
    let mut recipes = Vec::new();
    let mut skipped = Vec::new();
    for recipe in value
        .get("recipes")
        .and_then(Value::as_object)
        .map(|map| map.values().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let Some(name) = text(recipe.get("name")) else {
            continue;
        };
        let doc = text(recipe.get("doc"));
        if marker(doc.as_deref()) == Some("skip")
            || recipe.get("private").and_then(Value::as_bool) == Some(true)
        {
            skipped.push(name);
            continue;
        }
        let mut params = Vec::new();
        for param in recipe
            .get("parameters")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(name) = text(param.get("name")) else {
                continue;
            };
            let kind = text(param.get("kind")).unwrap_or_default();
            let default = text(param.get("default"));
            params.push(Param {
                name,
                variadic: matches!(kind.as_str(), "star" | "plus"),
                required: default.is_none() && kind != "star",
                default,
            });
        }
        recipes.push(Recipe { name, doc, params });
    }
    recipes.sort_by(|left, right| left.name.cmp(&right.name));
    skipped.sort();
    Ok(Dump {
        recipes,
        skipped,
        module_recipes: value
            .get("modules")
            .and_then(Value::as_object)
            .map_or(0, |modules| modules.values().filter_map(submodule).sum()),
    })
}

fn submodule(module: &Value) -> Option<usize> {
    let own = module
        .get("recipes")
        .and_then(Value::as_object)
        .map_or(0, Map::len);
    let nested = module
        .get("modules")
        .and_then(Value::as_object)
        .map_or(0, |modules| modules.values().filter_map(submodule).sum());
    Some(own.saturating_add(nested))
}

/// Invariant: just keeps only a recipe's last comment line as its doc, so `yi: read` and
/// `yi: skip` are that line — a marker riding above a description never reaches the dump.
fn marker(doc: Option<&str>) -> Option<&'static str> {
    match doc.map(str::trim) {
        Some(doc) if doc.eq_ignore_ascii_case(READ) => Some("read"),
        Some(doc) if doc.eq_ignore_ascii_case(SKIP) => Some("skip"),
        _ => None,
    }
}

fn read_by_name(name: &str) -> bool {
    let head = name.split(['-', '_']).next().unwrap_or(name);
    READ_NAMES.contains(&head)
}

fn kind(recipe: &Recipe) -> &'static str {
    match marker(recipe.doc.as_deref()) {
        Some(marker) => marker,
        None if read_by_name(&recipe.name) => "read",
        None => "exec",
    }
}

fn description(recipe: &Recipe) -> String {
    match recipe.doc.as_deref().map(str::trim) {
        Some(doc) if marker(Some(doc)).is_none() && !doc.is_empty() => doc.to_owned(),
        _ => format!("Run the project's just recipe {}", recipe.name),
    }
}

fn schema(recipe: &Recipe) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for param in &recipe.params {
        let mut spec = if param.variadic {
            json!({"type": "array", "items": {"type": "string"}})
        } else {
            json!({"type": "string"})
        };
        if let Some(default) = &param.default
            && let Some(map) = spec.as_object_mut()
        {
            map.insert("default".to_owned(), Value::String(default.clone()));
        }
        properties.insert(param.name.clone(), spec);
        if param.required {
            required.push(Value::String(param.name.clone()));
        }
    }
    let mut input = Map::new();
    input.insert("type".to_owned(), Value::String("object".to_owned()));
    input.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        input.insert("required".to_owned(), Value::Array(required));
    }
    Value::Object(input)
}

fn marker_line(justfile: &Path, recipe: &str) -> String {
    format!(
        "{MARKER}{}",
        json!({"justfile": justfile.to_string_lossy(), "recipe": recipe})
    )
}

fn wrapper(justfile: &Path, recipe: &Recipe) -> String {
    let declaration = json!({
        "name": recipe.name,
        "description": description(recipe),
        "input_schema": schema(recipe),
        "kind": kind(recipe),
    });
    let spec = json!({
        "justfile": justfile.to_string_lossy(),
        "recipe": recipe.name,
        "parameters": recipe
            .params
            .iter()
            .map(|param| json!([param.name, param.variadic]))
            .collect::<Vec<_>>(),
    });
    let quoted = spec.to_string().replace('\'', r"'\''");
    let name = &recipe.name;
    format!(
        "#!/bin/sh
{}
set -eu
if [ \"${{1:-}}\" = \"--schema\" ]; then
  cat <<'JUST_SCHEMA'
{declaration}
JUST_SCHEMA
  exit 0
fi
command -v python3 >/dev/null 2>&1 || {{
  printf '%s\\n' \"{name}: the just wrapper needs python3 to turn JSON arguments into argv\" >&2
  exit 127
}}
exec python3 -c '{ARGV}' '{quoted}'
",
        marker_line(justfile, name)
    )
}

fn marked(script: &str) -> Option<(PathBuf, String)> {
    let line = script.lines().find(|line| line.starts_with(MARKED))?;
    let value = parse(line.strip_prefix(MARKER)?).ok()?;
    Some((
        PathBuf::from(value.get("justfile")?.as_str()?),
        value.get("recipe")?.as_str()?.to_owned(),
    ))
}

fn reserved() -> Vec<String> {
    let mut names: Vec<String> = yi_runtime::builtin_tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    names.extend(WIRED.map(str::to_owned));
    names
}

#[cfg(unix)]
fn executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("could not make {} executable: {error}", path.display()))
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub(crate) fn sync(
    dir: &Path,
    justfile: &Path,
    dump: &str,
    picked: &[&str],
) -> Result<Report, String> {
    let Dump {
        recipes,
        skipped,
        module_recipes,
    } = recipes(dump)?;
    let chosen: Vec<&Recipe> = if picked.is_empty() {
        recipes.iter().collect()
    } else {
        picked
            .iter()
            .map(|name| {
                recipes
                    .iter()
                    .find(|recipe| recipe.name == *name)
                    .ok_or_else(|| match skipped.iter().any(|skipped| skipped == name) {
                        true => format!("{name} is private or marked `{SKIP}`"),
                        false => format!("the justfile has no recipe named {name}"),
                    })
            })
            .collect::<Result<_, _>>()?
    };
    let held = reserved();
    let clashes: Vec<&str> = chosen
        .iter()
        .map(|recipe| recipe.name.as_str())
        .filter(|name| held.iter().any(|held| held == name))
        .collect();
    if !clashes.is_empty() {
        return Err(format!(
            "{} is a built-in tool, so its just recipe is not imported",
            clashes.join(", ")
        ));
    }
    fs::create_dir_all(dir)
        .map_err(|error| format!("could not make {}: {error}", dir.display()))?;
    let mut report = Report {
        skipped,
        module_recipes,
        ..Report::default()
    };
    let mut written = Vec::new();
    for recipe in &chosen {
        let path = dir.join(&recipe.name);
        let body = wrapper(justfile, recipe);
        let existing = fs::read_to_string(&path).ok();
        if let Some(script) = &existing
            && marked(script).is_none()
        {
            return Err(format!(
                "{} is a hand-written tool; yi imports nothing over it",
                path.display()
            ));
        }
        match existing {
            Some(script) if script == body => report.unchanged.push(recipe.name.clone()),
            Some(_) => {
                write(&path, &body)?;
                report.updated.push(recipe.name.clone());
            }
            None => {
                write(&path, &body)?;
                report.added.push(recipe.name.clone());
            }
        }
        written.push(recipe.name.as_str());
    }
    if picked.is_empty() {
        report.removed = prune(dir, justfile, &written)?;
    }
    for names in [&mut report.added, &mut report.removed] {
        names.sort();
    }
    Ok(report)
}

fn write(path: &Path, body: &str) -> Result<(), String> {
    fs::write(path, body)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    executable(path)
}

fn prune(dir: &Path, justfile: &Path, kept: &[&str]) -> Result<Vec<String>, String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some((source, recipe)) = fs::read_to_string(&path).ok().as_deref().and_then(marked)
        else {
            continue;
        };
        if source != justfile || kept.contains(&recipe.as_str()) {
            continue;
        }
        fs::remove_file(&path)
            .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
        removed.push(recipe);
    }
    Ok(removed)
}

fn print(dir: &Path, justfile: &Path, report: &Report) {
    let imported = report.added.len() + report.updated.len() + report.unchanged.len();
    println!(
        "imported {imported} just recipes from {} into {}",
        justfile.display(),
        dir.display()
    );
    for (label, names) in [
        ("added", &report.added),
        ("updated", &report.updated),
        ("unchanged", &report.unchanged),
        ("removed", &report.removed),
        ("left out", &report.skipped),
    ] {
        if !names.is_empty() {
            println!("  {label} {}: {}", names.len(), names.join(", "));
        }
    }
    if report.module_recipes > 0 {
        println!(
            "  left out {} in just submodules: yi imports the justfile's own recipes",
            report.module_recipes
        );
    }
}
