//! `config.json` as the settings page edits it: every row with the file's value, and one
//! checked write that either lands whole or leaves the file as it was.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use yi_types::settings::{SettingKind, SettingRow, lookup, refusal, rows};

pub fn config_path(home: &Path) -> PathBuf {
    home.join(".yi/config.json")
}

fn read(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn current(home: &Path) -> Result<Vec<SettingRow>, String> {
    let config = read(&config_path(home))?;
    Ok(rows()
        .into_iter()
        .map(|mut row| {
            row.value = shown(&row.kind, lookup(&config, &row.id));
            row
        })
        .collect())
}

/// Incident: the owner once kept an API key under `keys`; a list or JSON value leaves this
/// process as its entry count, which is all the page shows of it.
fn shown(kind: &SettingKind, value: &Value) -> Value {
    match (kind, value) {
        (SettingKind::List | SettingKind::Json, Value::Array(items)) => Value::from(items.len()),
        (SettingKind::List | SettingKind::Json, Value::Object(fields)) => Value::from(fields.len()),
        (SettingKind::List | SettingKind::Json, Value::Null) => Value::Null,
        (SettingKind::List | SettingKind::Json, _) => Value::from(1),
        _ => value.clone(),
    }
}

/// One setting saved, `null` unsetting it; refused whole by its row's rule or the strict parse.
pub fn save(home: &Path, id: &str, value: Value) -> Result<Vec<SettingRow>, String> {
    let row = rows()
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| format!("no setting named {id}"))?;
    if let Some(why) = refusal(&row, &value) {
        return Err(why);
    }
    write(home, &[(id.split('.').collect(), value)])?;
    current(home)
}

/// The command an Open in app runs on `text`, saved first as markdown under `~/.yi/exports`:
/// each `{path}` argument becomes the file, or the file is appended when none names it.
pub fn open(home: &Path, name: &str, text: &str) -> Result<Vec<String>, String> {
    let path = config_path(home);
    let read = std::fs::read_to_string(&path).map_err(|error| error.to_string());
    let parsed =
        read.and_then(|raw| yi_types::config::parse(&raw).map_err(|error| error.to_string()));
    let (config, _) = parsed.map_err(|error| format!("{}: {error}", path.display()))?;
    let apps = config.open.unwrap_or_default();
    let named: Vec<_> = apps.iter().filter(|app| app.name == name).collect();
    let app = match named.as_slice() {
        [app] => app,
        [] => return Err(format!("no app named {name} under `open` in config.json")),
        _ => {
            return Err(format!(
                "two apps are named {name} under `open` in config.json"
            ));
        }
    };
    if app.command.is_empty() {
        return Err(format!("{name}'s command is empty"));
    }
    let dir = home.join(".yi/exports");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    prune(&dir);
    let n = STAGED.fetch_add(1, Ordering::Relaxed);
    let file = dir.join(format!(
        "{}-{}-{n}.md",
        yi_types::trace::now_us(),
        std::process::id()
    ));
    stage(&file, text, 0o600).map_err(|error| format!("{}: {error}", file.display()))?;
    let path = file.display().to_string();
    let mut argv: Vec<String> = app
        .command
        .iter()
        .map(|arg| arg.replace("{path}", &path))
        .collect();
    if !app.command.iter().any(|arg| arg.contains("{path}")) {
        argv.push(path);
    }
    Ok(argv)
}

/// How long a message saved for an app is kept: an app reads it when it opens, and a reply
/// may hold code or secrets that should not sit on disk.
const EXPORT_KEPT: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

fn prune(dir: &Path) {
    let Ok(listed) = std::fs::read_dir(dir) else {
        return;
    };
    for file in listed.flatten() {
        let old = file
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > EXPORT_KEPT);
        if old {
            let _ = std::fs::remove_file(file.path());
        }
    }
}

static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
static STAGED: AtomicU64 = AtomicU64::new(0);

/// Invariant: a write is read, edited and renamed under one lock, so two edits to different keys
/// both land; its staging name is its own, so `yi setup` running beside the daemon never shares it.
pub fn write(home: &Path, edits: &[(Vec<&str>, Value)]) -> Result<PathBuf, String> {
    let _held = WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = config_path(home);
    let mut root = read(&path)?;
    for (keys, value) in edits {
        set(&mut root, keys, value.clone())?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())? + "\n";
    yi_types::config::parse(&text)
        .map_err(|error| format!("that would write a config Yi cannot load: {error}"))?;
    let target = match (std::fs::canonicalize(&path), std::fs::read_link(&path)) {
        (Ok(target), _) => target,
        (Err(_), Ok(link)) => path.parent().ok_or("config path has no parent")?.join(link),
        (Err(_), Err(_)) => path.clone(),
    };
    let dir = target.parent().ok_or("config path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mode = std::fs::metadata(&target).map_or(0o600, |meta| {
        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777
    });
    let n = STAGED.fetch_add(1, Ordering::Relaxed);
    let staging = dir.join(format!(".config.json.{}.{n}", std::process::id()));
    let written = stage(&staging, &text, mode)
        .and_then(|()| std::fs::rename(&staging, &target))
        .map_err(|error| format!("{}: {error}", target.display()));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written.map(|()| path)
}

fn stage(staging: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(staging)?;
    std::fs::set_permissions(staging, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn set(root: &mut Value, keys: &[&str], value: Value) -> Result<(), String> {
    let (last, parents) = keys.split_last().ok_or("an empty key path")?;
    let mut node = root;
    for key in parents {
        node = node
            .as_object_mut()
            .ok_or_else(|| format!("`{key}` sits under a value that is not an object"))?
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let object = node
        .as_object_mut()
        .ok_or_else(|| format!("`{last}` sits under a value that is not an object"))?;
    if value.is_null() {
        object.remove(*last);
    } else {
        object.insert((*last).to_owned(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use serde_json::json;

    #[test]
    fn a_refused_save_leaves_the_file_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
        let home = Scratch::new("yi-settings-refused")?;
        let path = config_path(&home);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        let before = "{\"thinking\":\"high\",   \"gates\": {}}\n";
        std::fs::write(&path, before)?;
        let refused = save(&home, "rlm.maxDepth", json!(9));
        assert_eq!(
            refused.err().as_deref(),
            Some("rlm.maxDepth takes at most 3")
        );
        assert_eq!(std::fs::read_to_string(&path)?, before);
        let unknown = save(&home, "nope.key", json!(true));
        assert_eq!(unknown.err().as_deref(), Some("no setting named nope.key"));
        assert_eq!(std::fs::read_to_string(&path)?, before);
        Ok(())
    }

    /// Two consoles save through one daemon: without the lock a read-edit-rename lost the
    /// other key. Eight threads saving eight keys must leave all eight.
    #[test]
    fn concurrent_saves_of_different_keys_all_land() -> Result<(), Box<dyn std::error::Error>> {
        let home = Scratch::new("yi-settings-concurrent")?;
        let ids = [
            "kernel.prewarm",
            "telemetry.enabled",
            "console.autoSide",
            "mcp.enabled",
            "catalog.enabled",
            "lanes.enabled",
            "edit.freeformGrammar",
            "classifier.approve",
        ];
        std::thread::scope(|scope| {
            for id in ids {
                let home = &home;
                scope.spawn(move || save(home, id, json!(false)));
            }
        });
        let config: Value = serde_json::from_str(&std::fs::read_to_string(config_path(&home))?)?;
        for id in ids {
            assert_eq!(lookup(&config, id), &json!(false), "{id} was lost");
        }
        Ok(())
    }

    #[test]
    fn a_save_keeps_a_removed_key_and_null_unsets() -> Result<(), Box<dyn std::error::Error>> {
        let home = Scratch::new("yi-settings-keeps")?;
        let path = config_path(&home);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(&path, "{\"gates\": {\"x\": 1}, \"thinking\": \"low\"}\n")?;
        let rows = save(&home, "thinking", Value::Null)?;
        let thinking = rows
            .iter()
            .find(|row| row.id == "thinking")
            .ok_or("no row")?;
        assert_eq!(thinking.value, Value::Null);
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(config, json!({"gates": {"x": 1}}));
        Ok(())
    }

    /// Incident: the owner once stored an API key under `keys`; the page only ever learns how
    /// many entries a list or JSON row holds.
    #[test]
    fn a_secret_in_a_json_row_never_leaves_as_text() -> Result<(), Box<dyn std::error::Error>> {
        let home = Scratch::new("yi-settings-redacts")?;
        let path = config_path(&home);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        let config =
            r#"{"keys": {"openrouter": "sk-or-v1-abc"}, "skills": {"global": ["a", "b"]}}"#;
        std::fs::write(&path, config)?;
        let rows = current(&home)?;
        let wire = serde_json::to_string(&rows)?;
        assert!(!wire.contains("sk-or"), "the key reached the wire");
        let keys = rows
            .iter()
            .find(|row| row.id == "keys")
            .ok_or("no keys row")?;
        assert_eq!(keys.value, json!(1));
        let skills = rows
            .iter()
            .find(|row| row.id == "skills.global")
            .ok_or("no skills row")?;
        assert_eq!(skills.value, json!(2));
        Ok(())
    }

    fn with_apps(name: &str, apps: Value) -> Result<Scratch, Box<dyn std::error::Error>> {
        let home = Scratch::new(name)?;
        let path = config_path(&home);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(&path, json!({"open": apps}).to_string())?;
        Ok(home)
    }

    /// `{path}` may sit inside an argument; with none, the path is the last one. Two apps under
    /// one name would make the second unreachable, so the name is refused.
    #[test]
    fn an_app_gets_the_saved_message_where_its_command_says()
    -> Result<(), Box<dyn std::error::Error>> {
        let apps = json!([
            {"name": "inside", "command": ["edit", "--file={path}"]},
            {"name": "after", "command": ["open", "-R"]},
            {"name": "twice", "command": ["a"]},
            {"name": "twice", "command": ["b"]},
        ]);
        let home = with_apps("yi-open-argv", apps)?;
        let inside = open(&home, "inside", "é")?;
        let file = inside
            .get(1)
            .and_then(|arg| arg.strip_prefix("--file="))
            .ok_or("no path")?;
        assert_eq!(std::fs::read_to_string(file)?, "é");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(file)?.permissions().mode() & 0o777, 0o600);
        let after = open(&home, "after", "x")?;
        assert_eq!(after.len(), 3, "{after:?}");
        let twice = open(&home, "twice", "x");
        assert_eq!(
            twice.err().as_deref(),
            Some("two apps are named twice under `open` in config.json")
        );
        Ok(())
    }

    #[test]
    fn a_saved_message_older_than_a_week_is_pruned() -> Result<(), Box<dyn std::error::Error>> {
        let home = with_apps("yi-open-prune", json!([{"name": "a", "command": ["true"]}]))?;
        let dir = home.join(".yi/exports");
        std::fs::create_dir_all(&dir)?;
        let stale = dir.join("stale.md");
        let file = std::fs::File::create(&stale)?;
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(60);
        file.set_modified(old)?;
        open(&home, "a", "fresh")?;
        assert!(!stale.exists(), "a week-old export survived");
        assert_eq!(
            std::fs::read_dir(&dir)?.count(),
            1,
            "the fresh export is kept"
        );
        Ok(())
    }
}
