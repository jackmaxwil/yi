//! One table, not a plugin system: a lockfile names the toolchain.

use std::path::{Path, PathBuf};

use yi_types::lane::{SeenLockfiles, WarmReceipt};

use super::{LaneError, Pool, SlotIndex};

const SYNC_TIMEOUT_MS: u64 = 600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toolchain {
    pub lockfile: &'static str,
    pub sync: Option<&'static [&'static str]>,
    pub warm: &'static [&'static str],
}

pub const TOOLCHAINS: [Toolchain; 6] = [
    Toolchain {
        lockfile: "Cargo.lock",
        sync: None,
        warm: &["cargo", "check", "--offline", "--all-targets", "--quiet"],
    },
    Toolchain {
        lockfile: "pnpm-lock.yaml",
        sync: Some(&[
            "pnpm",
            "install",
            "--frozen-lockfile",
            "--offline",
            "--silent",
        ]),
        warm: &[
            "pnpm",
            "install",
            "--frozen-lockfile",
            "--offline",
            "--silent",
        ],
    },
    Toolchain {
        lockfile: "bun.lock",
        sync: Some(&["bun", "install", "--frozen-lockfile", "--silent"]),
        warm: &["bun", "install", "--frozen-lockfile", "--silent"],
    },
    Toolchain {
        lockfile: "bun.lockb",
        sync: Some(&["bun", "install", "--frozen-lockfile", "--silent"]),
        warm: &["bun", "install", "--frozen-lockfile", "--silent"],
    },
    Toolchain {
        lockfile: "package-lock.json",
        sync: Some(&[
            "npm",
            "ci",
            "--offline",
            "--no-audit",
            "--no-fund",
            "--silent",
        ]),
        warm: &[
            "npm",
            "ci",
            "--offline",
            "--no-audit",
            "--no-fund",
            "--silent",
        ],
    },
    Toolchain {
        lockfile: "uv.lock",
        sync: Some(&["uv", "sync", "--frozen", "--offline", "--quiet"]),
        warm: &["uv", "sync", "--frozen", "--offline", "--quiet"],
    },
];

pub fn detect(tree: &Path) -> Option<&'static Toolchain> {
    TOOLCHAINS
        .iter()
        .find(|toolchain| tree.join(toolchain.lockfile).is_file())
}

pub fn lockfile_hash(tree: &Path, toolchain: &Toolchain) -> Option<String> {
    let bytes = std::fs::read(tree.join(toolchain.lockfile)).ok()?;
    Some(crate::ext::content_hash(&String::from_utf8_lossy(&bytes)))
}

fn seen_path(pool: &Pool) -> PathBuf {
    pool.dir().join("seen.json")
}

fn read_seen(pool: &Pool) -> Result<SeenLockfiles, LaneError> {
    let path = seen_path(pool);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|source| LaneError::State { path, source })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SeenLockfiles::default()),
        Err(source) => Err(LaneError::Io { path, source }),
    }
}

fn mark_seen(pool: &Pool, hash: &str) -> Result<(), LaneError> {
    let mut seen = read_seen(pool)?;
    if seen.hashes.iter().any(|known| known == hash) {
        return Ok(());
    }
    seen.hashes.push(hash.to_owned());
    let path = seen_path(pool);
    let bytes = serde_json::to_vec(&seen).map_err(|source| LaneError::State {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, bytes).map_err(|source| LaneError::Io { path, source })
}

/// Seatbelt-contained like a bash call: slot and scratch writable, credentials unreadable.
fn contained(tree: &Path, home: &Path, argv: &[&str]) -> Option<(String, Vec<String>)> {
    let (program, args) = argv.split_first()?;
    if yi_tools::Sandbox::available() {
        let sandbox = yi_tools::Sandbox::for_workspace(tree, home, None);
        return Some(sandbox.wrap(program, args));
    }
    Some((
        (*program).to_owned(),
        args.iter().map(|arg| (*arg).to_owned()).collect(),
    ))
}

/// Invariant: a matching lockfile hash makes the claim git-only.
pub fn sync(pool: &Pool, slot: SlotIndex, tree: &Path) -> Result<Option<String>, LaneError> {
    let Some(toolchain) = detect(tree) else {
        return Ok(None);
    };
    let Some(hash) = lockfile_hash(tree, toolchain) else {
        return Ok(None);
    };
    let mut state = pool.read_state(slot)?;
    if state.lockfile.as_deref() == Some(hash.as_str()) {
        return Ok(None);
    }
    if let Some(argv) = toolchain.sync
        && let Some((program, args)) = contained(tree, pool.home(), argv)
    {
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        super::capture(
            tree,
            &program,
            &borrowed,
            std::time::Duration::from_millis(SYNC_TIMEOUT_MS),
        )
        .map_err(LaneError::Forge)?;
    }
    state.lockfile = Some(hash.clone());
    pool.write_state(slot, &state)?;
    mark_seen(pool, &hash)?;
    Ok(Some(toolchain.lockfile.to_owned()))
}

/// Invariant: the warmer only rebuilds a lockfile hash a session already synced.
pub fn warm(pool: &Pool, slot: SlotIndex) -> Result<(), LaneError> {
    let tree = pool.dir().join(slot.to_string());
    let Some(toolchain) = detect(&tree) else {
        return Ok(());
    };
    let Some(hash) = lockfile_hash(&tree, toolchain) else {
        return Ok(());
    };
    if !read_seen(pool)?.hashes.contains(&hash) {
        return Err(LaneError::LockfileUnseen { hash });
    }
    let mut state = pool.read_state(slot)?;
    let base = state.base.clone().unwrap_or_default();
    if state.warm.as_ref().is_some_and(|warm| {
        warm.base == base && warm.lockfile == hash && warm.exit_code.is_some_and(|code| code != 0)
    }) {
        // Invariant: a failed warm is not retried until the base moves.
        return Ok(());
    }
    let Some((program, args)) = contained(&tree, pool.home(), toolchain.warm) else {
        return Ok(());
    };
    let mut command = yi_tools::command("nice");
    command
        .arg("-n")
        .arg("19")
        .arg(program)
        .args(args)
        .current_dir(&tree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|source| LaneError::Io {
        path: tree.clone(),
        source,
    })?;
    state.warm = Some(WarmReceipt {
        base,
        lockfile: hash,
        pid: Some(child.id()),
        exit_code: None,
        started_ms: yi_session::now_ms(),
        extra: Default::default(),
    });
    pool.write_state(slot, &state)?;
    // Incident: an unwaited child is a zombie `kill -0` still sees; the claim waited 30 s on it.
    let reaper = pool.clone();
    std::thread::spawn(move || {
        let exit_code = child.wait().ok().and_then(|status| status.code());
        let Ok(mut state) = reaper.read_state(slot) else {
            return;
        };
        if let Some(warm) = state.warm.as_mut() {
            warm.pid = None;
            warm.exit_code = exit_code;
        }
        let _ = reaper.write_state(slot, &state);
    });
    Ok(())
}
