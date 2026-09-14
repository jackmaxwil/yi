//! The snapshot a verification is pinned to (plan section 6.3): the shadow gitdir tree when
//! the host has one, else a content hash of the workspace; and the tree the checker runs in.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::plan::canonical::Digest;

/// Files the default snapshotter hashes before it gives up on a workspace.
const TREE_HASH_FILE_CAP: usize = 20_000;

/// The tree id a verification is pinned to: the shadow gitdir when the host has one, else a
/// content hash of the workspace.
pub trait Snapshotter: Send + Sync {
    /// # Errors
    /// The workspace could not be read.
    fn capture(&self, workspace: &Path) -> Result<String, String>;

    /// # Errors
    /// The tree `id` names cannot be written into `into` (a fresh directory the checker runs in).
    fn materialize(&self, workspace: &Path, id: &str, into: &Path) -> Result<(), String>;
}

/// The shadow gitdir tree the turn checkpoints capture (plan section 6.3), `None` without git;
/// a plan store under the workspace is left out, since its checkpoint moves with every op.
pub fn shadow_tree(home: &Path, cwd: &Path, plans_dir: &Path) -> Option<Arc<dyn Snapshotter>> {
    let checkpoints =
        yi_tools::Checkpoints::open(&crate::checkpoint::checkpoint_root(home), cwd).ok()?;
    let exclude = match plans_dir.strip_prefix(cwd) {
        Ok(relative) => Some(relative.to_path_buf()),
        Err(_) => plans_dir.is_relative().then(|| plans_dir.to_path_buf()),
    };
    Some(Arc::new(ShadowTree {
        checkpoints,
        exclude,
    }))
}

struct ShadowTree {
    checkpoints: yi_tools::Checkpoints,
    exclude: Option<PathBuf>,
}

impl Snapshotter for ShadowTree {
    fn capture(&self, _workspace: &Path) -> Result<String, String> {
        let excluded: Vec<&Path> = self.exclude.iter().map(PathBuf::as_path).collect();
        self.checkpoints
            .capture_excluding(&excluded)
            .map(|tree| tree.as_str().to_owned())
            .map_err(|error| error.to_string())
    }

    fn materialize(&self, _workspace: &Path, id: &str, into: &Path) -> Result<(), String> {
        self.checkpoints
            .materialize(&yi_tools::TreeId::new(id), into)
            .map_err(|error| error.to_string())
    }
}

/// ponytail: a full walk hashing every file under the workspace bar `.git`, `.yi` and the plan
/// store, capped; the shadow gitdir honours ignore rules and is what git-backed surfaces install.
#[derive(Default)]
pub struct TreeHash {
    exclude: Option<PathBuf>,
}

impl TreeHash {
    /// A walk that skips the plan store wherever the workspace holds it.
    pub fn excluding(plans_dir: &Path) -> Self {
        Self {
            exclude: Some(
                std::fs::canonicalize(plans_dir).unwrap_or_else(|_| plans_dir.to_path_buf()),
            ),
        }
    }
}

fn excluded(path: &Path, exclude: Option<&Path>) -> bool {
    let Some(exclude) = exclude else { return false };
    path == exclude || std::fs::canonicalize(path).is_ok_and(|real| real == exclude)
}

fn walk(
    dir: &Path,
    prefix: &Path,
    exclude: Option<&Path>,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        if name == ".git" || name == ".yi" {
            continue;
        }
        let path = entry.path();
        let relative = prefix.join(&name);
        if path.is_dir() {
            if excluded(&path, exclude) {
                continue;
            }
            walk(&path, &relative, exclude, out)?;
        } else if path.is_file() {
            if out.len() >= TREE_HASH_FILE_CAP {
                return Err(format!(
                    "workspace exceeds {TREE_HASH_FILE_CAP} files; install the checkpoint snapshotter"
                ));
            }
            out.push((relative, path));
        }
    }
    Ok(())
}

/// The walked files, hashed in walk order; `copy_into` also writes each one under that root.
fn hash_tree(
    workspace: &Path,
    exclude: Option<&Path>,
    copy_into: Option<&Path>,
) -> Result<String, String> {
    let mut files = Vec::new();
    walk(workspace, Path::new(""), exclude, &mut files)?;
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest as _;
    for (relative, path) in files {
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(Digest::of(&bytes).as_bytes());
        if let Some(root) = copy_into {
            let target = root.join(&relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            std::fs::write(&target, &bytes)
                .map_err(|error| format!("{}: {error}", target.display()))?;
        }
    }
    Ok(format!("tree:{:x}", hasher.finalize()))
}

impl Snapshotter for TreeHash {
    fn capture(&self, workspace: &Path) -> Result<String, String> {
        hash_tree(workspace, self.exclude.as_deref(), None)
    }

    /// A content hash keeps no objects, so the copy is the workspace as it stands and is
    /// refused when that is no longer the tree the token names.
    fn materialize(&self, workspace: &Path, id: &str, into: &Path) -> Result<(), String> {
        std::fs::create_dir_all(into).map_err(|error| format!("{}: {error}", into.display()))?;
        let copied = hash_tree(workspace, self.exclude.as_deref(), Some(into))?;
        if copied != id {
            return Err(format!(
                "the workspace is {copied}, not the snapshot {id} the verification was pinned to"
            ));
        }
        Ok(())
    }
}
