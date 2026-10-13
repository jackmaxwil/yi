use std::error::Error;
use std::path::Path;

use serde_json::Value;

/// The prompt slot names each session under `sessions` recorded, one list per session file.
pub fn prompt_slots(sessions: &Path) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let mut found = Vec::new();
    for project in std::fs::read_dir(sessions)? {
        for path in std::fs::read_dir(project?.path())? {
            let path = path?.path();
            if !path.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let mut names = Vec::new();
            for line in std::fs::read_to_string(&path)?.lines() {
                let entry: Value = serde_json::from_str(line)?;
                if entry["customType"] == "ext_state" {
                    let slots = entry["data"]["slots"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    names.extend(
                        slots
                            .iter()
                            .filter_map(|slot| slot["name"].as_str().map(str::to_owned)),
                    );
                }
            }
            found.push(names);
        }
    }
    Ok(found)
}
