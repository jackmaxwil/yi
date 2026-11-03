//! What the host hands one contained spawn besides its profile.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// `env` rides `Command::env`, never `argv`, which `ps` reads, so it may hold a secret; `assign` rides
/// `env`'s argv past the scrub, so it never does. `after` returns rows the model sees beside the output.
pub struct Spawn {
    pub env: Vec<(String, String)>,
    pub assign: Vec<String>,
    pub after: Box<dyn FnOnce() -> Vec<String> + Send>,
}

pub type SpawnFn = Arc<dyn Fn(&str) -> Result<Spawn, SpawnRefused> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnRefused {
    #[error("[not started: {why}; contained commands reach the network only through it]")]
    Egress { why: String },
}

/// A fresh tree's work the host owes before its first command: a toolchain sync, a cargo carry.
pub type FirstUse = Box<dyn FnOnce() -> Vec<String> + Send>;
type Hook = Arc<Mutex<Option<FirstUse>>>;

fn fresh_trees() -> &'static Mutex<HashMap<PathBuf, Hook>> {
    static TREES: OnceLock<Mutex<HashMap<PathBuf, Hook>>> = OnceLock::new();
    TREES.get_or_init(Mutex::default)
}

/// The entry stays until [`forget_first_use`], since [`in_fresh_tree`] outlives the hook.
pub fn register_first_use(root: &Path, hook: FirstUse) {
    if let Ok(mut trees) = fresh_trees().lock() {
        trees.insert(root.to_path_buf(), Arc::new(Mutex::new(Some(hook))));
    }
}

pub fn forget_first_use(root: &Path) {
    if let Ok(mut trees) = fresh_trees().lock() {
        trees.remove(root);
    }
}

fn hook_for(cwd: &Path) -> Option<Hook> {
    let trees = fresh_trees().lock().ok()?;
    let found = trees.iter().find(|(root, _)| cwd.starts_with(root));
    found.map(|(_, hook)| Arc::clone(hook))
}

/// Whether `cwd` is under a tree a child forked: cargo there must not share a parent's target.
pub(crate) fn in_fresh_tree(cwd: &Path) -> bool {
    hook_for(cwd).is_some()
}

/// Parallel first commands in one tree wait here, so the hook runs once. Its failure is a row and
/// never a failed command: the child rebuilds what it did not carry.
pub(crate) fn run_first_use(cwd: &Path) -> Vec<String> {
    let Some(hook) = hook_for(cwd) else {
        return Vec::new();
    };
    let Ok(mut slot) = hook.lock() else {
        return Vec::new();
    };
    slot.take().map(|first_use| first_use()).unwrap_or_default()
}
