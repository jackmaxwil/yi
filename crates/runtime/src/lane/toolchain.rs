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

pub fn detect(tree: &Path) -> Vec<&'static Toolchain> {
    TOOLCHAINS
        .iter()
        .filter(|toolchain| tree.join(toolchain.lockfile).is_file())
        .collect()
}

fn lockfiles_key(tree: &Path, toolchains: &[&Toolchain]) -> Option<String> {
    let hashes = toolchains
        .iter()
        .map(|toolchain| lockfile_hash(tree, toolchain))
        .collect::<Option<Vec<_>>>()?;
    match &hashes[..] {
        [] => None,
        [one] => Some(one.clone()),
        many => Some(crate::ext::content_hash(&many.join("\n"))),
    }
}

pub fn lockfile_hash(tree: &Path, toolchain: &Toolchain) -> Option<String> {
    let bytes = std::fs::read(tree.join(toolchain.lockfile)).ok()?;
    Some(crate::ext::content_hash(&String::from_utf8_lossy(&bytes)))
}

/// An interpreter's script counts by its stem, so `node …/npm ci` is npm's warmer; the warm
/// command's own arguments must follow it, so the user's `cargo build` is not one.
pub(super) fn is_warmer(tree: &Path, argv: &[String]) -> bool {
    detect(tree).iter().any(|toolchain| {
        let Some((program, flags)) = toolchain.warm.split_first() else {
            return false;
        };
        argv.iter().enumerate().any(|(at, arg)| {
            Path::new(arg).file_stem().and_then(|stem| stem.to_str()) == Some(*program)
                && argv.get(at.saturating_add(1)..).is_some_and(|rest| {
                    rest.len() >= flags.len() && rest.iter().zip(flags).all(|(a, f)| a == f)
                })
        })
    })
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

pub(super) fn mark_seen(pool: &Pool, hash: &str) -> Result<(), LaneError> {
    let _pool = pool.lock()?;
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
    yi_session::replace_file(&path, &bytes).map_err(|source| LaneError::Io { path, source })
}

/// The same argv under the same profile: `contained` for a call that builds one, `wrapped` for a
/// caller that reads several entries under one profile.
pub(super) fn wrapped(
    argv: &[&str],
    sandbox: Option<&yi_tools::Sandbox>,
) -> Option<(String, Vec<String>)> {
    let (program, args) = argv.split_first()?;
    Some(match sandbox {
        Some(sandbox) => sandbox.wrap(program, args),
        None => (
            (*program).to_owned(),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
        ),
    })
}

/// Seatbelt-contained like a bash call: slot and scratch writable, credentials unreadable.
/// The sandbox rides along so a failed sync can carry the real refusal, not a guess from its text.
fn contained(
    tree: &Path,
    home: &Path,
    argv: &[&str],
) -> Option<(String, Vec<String>, Option<yi_tools::Sandbox>)> {
    let sandbox =
        yi_tools::Sandbox::available().then(|| yi_tools::Sandbox::for_workspace(tree, home, None));
    let (program, args) = wrapped(argv, sandbox.as_ref())?;
    Some((program, args, sandbox))
}

/// Invariant: a matching lockfile hash makes the claim git-only; every lockfile present syncs.
pub fn sync(pool: &Pool, slot: SlotIndex, tree: &Path) -> Result<Option<String>, LaneError> {
    let _span = yi_types::trace::span("lane.toolchain_sync");
    let toolchains = detect(tree);
    let Some(hash) = lockfiles_key(tree, &toolchains) else {
        return Ok(None);
    };
    let mut state = pool.read_state(slot)?;
    if state.lockfile.as_deref() == Some(hash.as_str()) {
        return Ok(None);
    }
    for toolchain in &toolchains {
        let Some(argv) = toolchain.sync else {
            continue;
        };
        let Some((program, args, sandbox)) = contained(tree, pool.home(), argv) else {
            continue;
        };
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        super::capture_named(
            tree,
            &argv.join(" "),
            &program,
            &borrowed,
            std::time::Duration::from_millis(SYNC_TIMEOUT_MS),
            yi_tools::OUTPUT_CAP,
            sandbox.as_ref(),
        )
        .map_err(LaneError::Forge)?;
    }
    state.lockfile = Some(hash.clone());
    pool.write_state(slot, &state)?;
    mark_seen(pool, &hash)?;
    let names: Vec<&str> = toolchains
        .iter()
        .map(|toolchain| toolchain.lockfile)
        .collect();
    Ok(Some(names.join(", ")))
}

/// Invariant: the warmer only rebuilds a lockfile hash a session already synced.
pub fn warm(pool: &Pool, slot: SlotIndex) -> Result<(), LaneError> {
    let tree = pool.dir().join(slot.to_string());
    let toolchains = detect(&tree);
    let (Some(toolchain), Some(hash)) = (toolchains.first(), lockfiles_key(&tree, &toolchains))
    else {
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
    let Some((program, args, _)) = contained(&tree, pool.home(), toolchain.warm) else {
        return Ok(());
    };
    let mut command = yi_tools::keyless_command("nice");
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

impl Pool {
    /// Incident: a killed install leaves half a tree, so the lockfile hash is forgotten too.
    pub(super) fn stop_warmer(&self, slot: SlotIndex) -> Result<(), LaneError> {
        let mut state = self.read_state(slot)?;
        let Some(pid) = state.warm.as_ref().and_then(|warm| warm.pid) else {
            return Ok(());
        };
        let running =
            || yi_kernel::bootstrap::process_is_running(pid).map_err(super::io_error(&self.dir));
        if !running()? {
            return Ok(());
        }
        // Incident: a receipt outlives its warmer, and its pid can name an unrelated process.
        let tree = self.slot_path(slot);
        if !process_argv(&self.dir, pid).is_some_and(|argv| is_warmer(&tree, &argv)) {
            if let Some(warm) = state.warm.as_mut() {
                warm.pid = None;
            }
            state.lockfile = None;
            return self.write_state(slot, &state);
        }
        let term = ["-c", r#"kill -TERM "$1""#, "kill", &pid.to_string()];
        let _the_poll_below_sees_a_failed_term =
            super::capture(&self.dir, "/bin/sh", &term, super::probe_deadline());
        let started = std::time::Instant::now();
        let _span = yi_types::trace::span("lane.warmer_exit");
        let mut pace = yi_types::backoff::backoff(std::time::Duration::from_millis(200));
        while running()? {
            if started.elapsed().as_millis() >= u128::from(super::WARMER_EXIT_WAIT_MS) {
                return Err(LaneError::WarmerStuck {
                    slot,
                    pid,
                    waited_ms: super::WARMER_EXIT_WAIT_MS,
                });
            }
            std::thread::sleep(pace());
        }
        if let Some(warm) = state.warm.as_mut() {
            warm.pid = None;
        }
        state.lockfile = None;
        self.write_state(slot, &state)
    }
}

/// The argv `pid` runs now, or `None` when it cannot be read; slim Linux images ship no ps(1).
fn process_argv(cwd: &Path, pid: u32) -> Option<Vec<String>> {
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let text = String::from_utf8_lossy(&cmdline);
        return Some(
            text.split('\0')
                .filter(|arg| !arg.is_empty())
                .map(str::to_owned)
                .collect(),
        );
    }
    let pid = pid.to_string();
    let args = super::capture(
        cwd,
        "/bin/ps",
        &["-o", "args=", "-p", &pid],
        super::probe_deadline(),
    )
    .ok()?;
    Some(args.split_whitespace().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    /// #906 review: a sync runs the repo's install hooks on the host and a failed one hands back
    /// their output; where no sandbox scrubbed it, yi's provider keys were in that output.
    #[test]
    fn a_failed_sync_prints_no_provider_key() -> Result<(), Box<dyn std::error::Error>> {
        const NAME: &str = "lane::toolchain::tests::a_failed_sync_prints_no_provider_key";
        const KEY: &str = "ENV-906-OPENAI";
        if std::env::var_os("OPENAI_API_KEY").is_none_or(|key| key != KEY) {
            let rerun = yi_tools::command(std::env::current_exe()?)
                .args(["--exact", NAME])
                .env("OPENAI_API_KEY", KEY)
                .output()?;
            let said =
                String::from_utf8_lossy(&rerun.stdout) + String::from_utf8_lossy(&rerun.stderr);
            assert!(
                rerun.status.success() && said.contains("1 passed"),
                "{said}"
            );
            return Ok(());
        }
        let hook = ["-c", "printenv OPENAI_API_KEY; exit 1"];
        let ran = super::super::capture(
            &std::env::temp_dir(),
            "sh",
            &hook,
            std::time::Duration::from_secs(10),
        );
        let error = ran.err().ok_or("the hook exits 1")?;
        assert!(error.contains("failed") && !error.contains(KEY), "{error}");
        Ok(())
    }
}
