use std::error::Error;

use std::path::{Path, PathBuf};

/// Every session JSONL under `sessions/<project>/`. Beside them sit directories that are not
/// sessions: `family/` and `kernels/` under `sessions/`, `schedules/<id>/` in each project.
pub fn session_files(sessions: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for project in std::fs::read_dir(sessions)? {
        for entry in std::fs::read_dir(project?.path())? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

pub fn recursive_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(at)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

#[allow(dead_code)]
pub fn session_text(sessions: &Path) -> std::io::Result<String> {
    session_files(sessions)?
        .iter()
        .map(std::fs::read_to_string)
        .collect::<std::io::Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

pub fn frame_entries(dir: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

pub fn frames_text(dir: &Path) -> Result<String, Box<dyn Error>> {
    let entries = frame_entries(dir)?;
    assert!(!entries.is_empty(), "at least one frame must be dumped");
    let mut frames = String::new();
    for entry in entries {
        frames.push_str(&std::fs::read_to_string(entry.path())?);
    }
    Ok(frames)
}
