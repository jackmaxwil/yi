//! What a contained process may write of a git checkout: the files the host's next git runs,
//! the directories that hold admin dirs and submodules, and a lane's own refs.

use std::path::{Path, PathBuf};

/// What the host's next git runs or reads from a git dir: `hooks` and both configs, `info` and an
/// alternate; the pointers its policy is built from; origin's `HEAD`, which names trunk.
pub(super) const HOST_RUN_BY_GIT: [&str; 8] = [
    "refs/remotes/origin/HEAD",
    "hooks",
    "config",
    "config.worktree",
    "info",
    "objects/info/alternates",
    "commondir",
    "gitdir",
];

/// A linked worktree's git writes its admin dir, the objects, its own branch and the stash, and
/// locks `packed-refs` to delete a ref. Incident: with the whole common dir, a lane deleted a sibling's.
pub(super) fn lane_writes(cwd: &Path, git_dirs: &[PathBuf], home: &Path) -> Vec<PathBuf> {
    let [admin, common, ..] = git_dirs else {
        return git_dirs.to_vec();
    };
    let refs = |name: &str| {
        [
            common.join(name),
            common.join(format!("{name}.lock")),
            common.join("logs").join(name),
            common.join("logs").join(format!("{name}.lock")),
        ]
    };
    let branches = own_branches(cwd, home);
    let leaves: Vec<PathBuf> = (branches.iter())
        .map(|branch| format!("refs/heads/{branch}"))
        .chain(["refs/stash".to_owned()])
        .flat_map(|name| refs(&name))
        .collect();
    for parent in leaves.iter().filter_map(|leaf| leaf.parent()) {
        let _best_effort = std::fs::create_dir_all(parent);
    }
    (leaves.into_iter())
        .chain([
            admin.clone(),
            common.join("objects"),
            common.join("packed-refs.lock"),
        ])
        .collect()
}

/// The branch a host-owned record gives this tree, and nothing else. Invariant: `HEAD` is
/// contained-writable, so a tree whose record does not parse has no branch this profile may grant.
fn own_branches(cwd: &Path, home: &Path) -> Vec<String> {
    let record = [home.join(".yi/lanes"), home.join(".yi/forks")]
        .iter()
        .any(|pool| cwd.starts_with(pool))
        .then(|| std::fs::read(cwd.with_extension("json")).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<yi_types::lane::SlotState>(&bytes).ok())
        .and_then(|state| state.session)
        .map(|session| yi_types::lane::branch_for_session(&session));
    record.into_iter().collect()
}

/// Module git dirs one profile build opens; past it none is, which only narrows what a submodule
/// may write.
const MODULE_WALK_MAX: usize = 256;

/// A git directory closed whole to writes, and the roots inside it this session still writes: a
/// contained process creates an admin dir or a module under a writable root, so the wall sits on
/// the directory, never on the names one build found there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub denied: PathBuf,
    pub open: Vec<PathBuf>,
}

/// The two directories under `git_dirs` that hold admin dirs and submodules: a lane keeps its own
/// admin dir and the module dirs it already has, and a name created after the build is sealed.
pub(super) fn sealed(git_dirs: &[PathBuf]) -> Vec<Sealed> {
    let mut dirs: Vec<PathBuf> = git_dirs.to_vec();
    dirs.extend((git_dirs.iter()).flat_map(|dir| module_dirs(&dir.join("modules"))));
    let keeper = |parent: &Path| -> Vec<PathBuf> {
        (dirs.iter())
            .filter(|dir| dir.starts_with(parent))
            .cloned()
            .collect()
    };
    let mut sealed: Vec<Sealed> = dirs
        .iter()
        .map(|dir| dir.join("worktrees"))
        .map(|denied| Sealed {
            open: keeper(&denied),
            denied,
        })
        .collect();
    sealed.extend(
        dirs.iter()
            .map(|dir| dir.join("modules"))
            .map(|denied| Sealed {
                open: keeper(&denied),
                denied,
            }),
    );
    sealed
}

/// The module git dirs under `modules`, nested submodules included: a module's own object store
/// stays writable, and one the build did not see is covered by the seal above it. Incident: a
/// walk through every object store cost 256 directories before the first submodule was found.
fn module_dirs(modules: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut walk = vec![modules.to_path_buf()];
    let mut visited = 0usize;
    while let Some(parent) = walk.pop() {
        for child in directories(&parent) {
            visited = visited.saturating_add(1);
            if visited > MODULE_WALK_MAX {
                return found;
            }
            // A module's dir is named after its path, so `modules/nested/sub` sits under a dir
            // that is no module; only a dir holding HEAD is one, and only its own modules are
            // walked on, never the object store beside them.
            if child.join("HEAD").is_file() {
                found.push(child.clone());
                walk.push(child.join("modules"));
            } else {
                walk.push(child);
            }
        }
    }
    found
}

/// The real directories in `dir`, never a link: a contained process writes some of these trees.
fn directories(dir: &Path) -> impl Iterator<Item = PathBuf> {
    (std::fs::read_dir(dir).into_iter().flatten().flatten())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
}
