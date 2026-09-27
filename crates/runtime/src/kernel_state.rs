use std::path::{Path, PathBuf};

pub(crate) fn state_dir(dir: &Path, per_session: bool, key: Option<&str>) -> Option<PathBuf> {
    if !per_session {
        return Some(dir.to_path_buf());
    }
    let id = key.filter(|id| yi_session::validate_session_id(id).is_ok())?;
    let state = dir.join("kernels").join(id);
    std::fs::create_dir_all(&state).ok()?;
    let (old, new) = (
        crate::kernel::snapshot_paths(dir, Some(id)),
        crate::kernel::snapshot_paths(&state, Some(id)),
    );
    for (from, to) in [(old.0, new.0), (old.1, new.1)] {
        if from.is_file() && !to.exists() {
            let _unmoved_stays_where_no_kernel_reads = std::fs::rename(&from, &to);
        }
    }
    Some(state)
}
