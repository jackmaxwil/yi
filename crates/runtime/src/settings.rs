//! `config.json` as the settings page edits it: every row with the file's value, and one
//! checked write that either lands whole or leaves the file as it was.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use yi_types::settings::{SettingRow, lookup, refusal, rows};

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
            row.value = lookup(&config, &row.id).clone();
            row
        })
        .collect())
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

static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
static STAGED: AtomicU64 = AtomicU64::new(0);

/// Invariant: a write is read, edited and renamed under one lock with its own staging name,
/// so two edits to different keys both land; the shared `.config.json.setup` clobbered one.
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

    /// Incident: every write staged at one `.config.json.setup`, so two at once clobbered
    /// each other; eight threads saving eight keys must leave all eight.
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
}
