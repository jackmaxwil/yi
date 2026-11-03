//! The refute: a green check is trusted only after it turns red with the artifacts emptied
//! and with the baseline restored, with the current artifacts put back on every path.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use yi_types::spec::{
    Artifact, Frozen, Kind, Mutant, RefuteKey, RefuteRecord, ReqId, RunKind, SpecError, Status,
};

use super::{SpecStore, copy_tree, run};

fn io(path: &Path, source: std::io::Error) -> SpecError {
    SpecError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn remove_path(path: &Path) -> Result<(), SpecError> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| io(path, e))
    } else {
        std::fs::remove_file(path).map_err(|e| io(path, e))
    }
}

/// `rename` when the two paths share a filesystem, a copy and a remove when they do not.
fn move_path(from: &Path, to: &Path) -> Result<(), SpecError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_tree(from, to)?;
    remove_path(from)
}

/// The current artifacts, moved aside for a mutant run. `restore` puts them back and is
/// called on every path; `Drop` restores what is still aside as the last resort.
pub struct Aside {
    moves: Vec<(PathBuf, PathBuf)>,
    restored: bool,
}

impl Aside {
    pub fn take(artifacts: &[Artifact], dir: &Path) -> Result<Self, SpecError> {
        recover(artifacts, dir)?;
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        let mut aside = Self {
            moves: Vec::new(),
            restored: false,
        };
        for (index, artifact) in artifacts.iter().enumerate() {
            if !artifact.path().exists() {
                continue;
            }
            let to = dir.join(index.to_string());
            move_path(artifact.path(), &to)?;
            aside.moves.push((artifact.path().to_path_buf(), to));
        }
        Ok(aside)
    }

    /// Whether the artifact at `index` was a directory when it was moved aside.
    fn was_dir(&self, artifact: &Artifact) -> Option<bool> {
        self.moves
            .iter()
            .find(|(original, _)| original == artifact.path())
            .map(|(_, aside)| aside.is_dir())
    }

    pub fn restore(mut self) -> Result<(), SpecError> {
        self.restore_all()
    }

    fn restore_all(&mut self) -> Result<(), SpecError> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let mut first = None;
        for (original, aside) in self.moves.iter().rev() {
            let put_back = remove_path(original).and_then(|()| move_path(aside, original));
            if let Err(error) = put_back
                && first.is_none()
            {
                first = Some(error);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

impl Drop for Aside {
    fn drop(&mut self) {
        let _a_drop_restores_as_the_last_resort = self.restore_all();
    }
}

/// Invariant: a copy an interrupted refute left goes back over the path it came from before the
/// directory is reused; deleting it there deletes the only copy of the user's product.
pub fn recover(artifacts: &[Artifact], dir: &Path) -> Result<(), SpecError> {
    if !dir.exists() {
        return Ok(());
    }
    for (index, artifact) in artifacts.iter().enumerate() {
        let aside = dir.join(index.to_string());
        if aside.exists() {
            remove_path(artifact.path())?;
            move_path(&aside, artifact.path())?;
        }
    }
    remove_path(dir)
}

/// Install one mutant at an artifact's path: the baseline copy (or absence), or an empty
/// file or directory of the same shape the artifact had.
fn install(
    mutant: Mutant,
    index: usize,
    artifact: &Artifact,
    snapshot: &Path,
    aside: &Aside,
) -> Result<(), SpecError> {
    let path = artifact.path();
    remove_path(path)?;
    let baseline = snapshot.join(index.to_string());
    match mutant {
        Mutant::Baseline => {
            if baseline.exists() {
                copy_tree(&baseline, path)?;
            }
            Ok(())
        }
        Mutant::Empty => {
            let dir = aside.was_dir(artifact).unwrap_or_else(|| baseline.is_dir());
            if dir {
                std::fs::create_dir_all(path).map_err(|e| io(path, e))
            } else {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                }
                std::fs::write(path, b"").map_err(|e| io(path, e))
            }
        }
    }
}

/// The file the command names, when it is one in the cwd: `python3 check.py` → `check.py`.
pub fn check_file(cwd: &Path, command: &str) -> Option<PathBuf> {
    command
        .split_whitespace()
        .map(|token| cwd.join(token))
        .find(|path| path.is_file())
}

pub fn key_of(cwd: &Path, frozen: &Frozen) -> RefuteKey {
    let command = frozen.draft.check.as_str().to_owned();
    let check_hash = check_file(cwd, &command)
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
    RefuteKey {
        command,
        check_hash,
        artifacts: frozen.draft.artifacts.clone(),
    }
}

/// Run both mutants and record the `f2p` ids that stayed green under each. A check that
/// cannot run without the artifact proved its dependence: not green.
pub fn refute(store: &SpecStore, frozen: &Frozen) -> Result<RefuteRecord, SpecError> {
    let key = key_of(store.cwd(), frozen);
    let aside_dir = store.dir().join("aside");
    let mut record = RefuteRecord {
        at: yi_session::now_ms(),
        key,
        baseline_green: Vec::new(),
        empty_green: Vec::new(),
        extra: serde_json::Map::new(),
    };
    for mutant in [Mutant::Baseline, Mutant::Empty] {
        if let Some(artifact) = frozen
            .draft
            .artifacts
            .iter()
            .find(|artifact| artifact.escapes(store.cwd()))
        {
            return Err(SpecError::Outside(artifact.path().to_path_buf()));
        }
        let aside = Aside::take(&frozen.draft.artifacts, &aside_dir)?;
        let installed =
            frozen
                .draft
                .artifacts
                .iter()
                .enumerate()
                .try_for_each(|(index, artifact)| {
                    install(mutant, index, artifact, &frozen.snapshot, &aside)
                });
        let run = installed.map(|()| {
            run::run_check(
                &frozen.draft.check,
                store.cwd(),
                frozen.timeout,
                RunKind::Refute(mutant),
                store.sandbox(),
            )
        });
        aside.restore()?;
        let run = run?;
        let green: Vec<ReqId> = frozen
            .draft
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == Kind::F2p)
            .filter(|requirement| run.status(&requirement.id) == Status::Green)
            .map(|requirement| requirement.id.clone())
            .collect();
        match mutant {
            Mutant::Baseline => record.baseline_green = green,
            Mutant::Empty => record.empty_green = green,
        }
    }
    Ok(record)
}
