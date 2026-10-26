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

#[allow(dead_code)]
pub fn session_text(sessions: &Path) -> std::io::Result<String> {
    session_files(sessions)?
        .iter()
        .map(std::fs::read_to_string)
        .collect::<std::io::Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}
