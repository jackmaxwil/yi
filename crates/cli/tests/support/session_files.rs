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
