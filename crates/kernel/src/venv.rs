//! Kernel environments: a dependency venv named by what it installs, an overlay per runtime
//! identity inside it, and a sweep that spares any one a kernel runs from or a bootstrap holds.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::bootstrap::{
    BOOTSTRAP_SCHEMA, BootstrapOptions, DEFAULT_RLM_EXTRA_UV_ARGS, IPYKERNEL_REQUIREMENT,
    PYTHON_SKILLS, STATE_SNAPSHOT_REQUIREMENT, default_runtime_source_dir,
    default_skills_source_dir, env_path,
};
use crate::lock::{BOOTSTRAP_LOCK_NAME, try_bootstrap_lock};

const SWEEP_AFTER_SECS: u64 = 86_400;
const OVERLAYS: &str = "overlays";
const OVERLAY_PREFIX: &str = "rt-";

/// The `dependencies = [...]` array of a pyproject, or the whole file when it has none.
fn dependency_list(text: &str) -> &str {
    text.split_once("\ndependencies")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map_or(text, |(list, _)| list)
}

fn dependency_lists(runtime_dir: &Path, skills_dir: &Path) -> Vec<String> {
    let skills = PYTHON_SKILLS
        .iter()
        .map(|(_, subdir)| skills_dir.join(subdir));
    std::iter::once(runtime_dir.to_path_buf())
        .chain(skills)
        .map(|dir| {
            std::fs::read_to_string(dir.join("pyproject.toml"))
                .map(|text| dependency_list(&text).to_owned())
                .unwrap_or_default()
        })
        .collect()
}

/// Incident: one `~/.yi/kernel-venv` served every commit, and a changed pin rebuilt it in place
/// under the sessions importing from it. What the dependency venv installs names it now.
pub(crate) fn venv_name(pins: &str, extras: &[&str], dependencies: &[String]) -> String {
    let mut hash = Sha256::new_with_prefix(pins.as_bytes());
    for extra in extras {
        hash.update(b"\0");
        hash.update(extra.as_bytes());
    }
    for list in dependencies {
        hash.update(b"\0\0");
        hash.update(list.as_bytes());
    }
    let digest = hash.finalize();
    let slot: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("kernel-venv-{slot}")
}

fn default_name(runtime_dir: &Path, skills_dir: &Path) -> String {
    venv_name(
        &format!("{BOOTSTRAP_SCHEMA}\0{IPYKERNEL_REQUIREMENT}\0{STATE_SNAPSHOT_REQUIREMENT}"),
        &DEFAULT_RLM_EXTRA_UV_ARGS,
        &dependency_lists(runtime_dir, skills_dir),
    )
}

fn named_venv_dir(home: &Path, runtime_dir: &Path, skills_dir: &Path) -> PathBuf {
    match env_path("YI_KERNEL_VENV") {
        Some(dir) => dir,
        None => home.join(".yi").join(default_name(runtime_dir, skills_dir)),
    }
}

/// The dependency venv for the shipped package; the kernel runs from an overlay inside it.
pub fn kernel_venv_dir(home: &Path) -> PathBuf {
    named_venv_dir(
        home,
        &default_runtime_source_dir(),
        &default_skills_source_dir(),
    )
}

pub(crate) fn xdg_kernel_venv_dir(home: &Path, runtime_dir: &Path, skills_dir: &Path) -> PathBuf {
    let data_home = match env_path("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => home.join(".local").join("share"),
    };
    data_home
        .join("yi")
        .join(default_name(runtime_dir, skills_dir))
}

pub(crate) fn resolve_writable_venv_dir(options: &BootstrapOptions) -> Result<PathBuf, String> {
    let home = &options.home;
    let (runtime, skills) = (&options.runtime_source_dir, &options.skills_source_dir);
    let primary = options
        .venv_dir
        .clone()
        .unwrap_or_else(|| named_venv_dir(home, runtime, skills));
    let parent = primary.parent().unwrap_or(&primary);
    match std::fs::create_dir_all(parent) {
        Ok(()) => Ok(primary),
        Err(primary_error) => {
            if options.venv_dir.is_some() || env_path("YI_KERNEL_VENV").is_some() {
                return Err(format!(
                    "couldn't create kernel venv parent directory for {}: {primary_error}",
                    primary.display()
                ));
            }
            let fallback = xdg_kernel_venv_dir(home, runtime, skills);
            let fallback_parent = fallback.parent().unwrap_or(&fallback);
            std::fs::create_dir_all(fallback_parent).map_err(|fallback_error| {
                format!(
                    "couldn't create kernel venv directory at {} or {}; set YI_KERNEL_PYTHON to a python with ipykernel installed. {fallback_error}",
                    primary.display(),
                    fallback.display()
                )
            })?;
            Ok(fallback)
        }
    }
}

/// The overlay one runtime identity installs into, inside the dependency venv it reads.
pub(crate) fn overlay_dir(deps: &Path, identity: &str) -> PathBuf {
    let hex = identity.strip_prefix("sha256:").unwrap_or(identity);
    let slot: String = hex.chars().take(12).collect();
    deps.join(OVERLAYS).join(format!("{OVERLAY_PREFIX}{slot}"))
}

pub(crate) fn site_packages(venv: &Path) -> Result<PathBuf, String> {
    let lib = venv.join("lib");
    std::fs::read_dir(&lib)
        .map_err(|error| format!("{}: {error}", lib.display()))?
        .flatten()
        .map(|entry| entry.path().join("site-packages"))
        .find(|site| site.is_dir())
        .ok_or_else(|| format!("no site-packages under {}", lib.display()))
}

fn running_commands() -> Option<String> {
    let output = crate::bootstrap::command(Path::new("ps"))
        .args(["-Aww", "-o", "command="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[expect(
    clippy::disallowed_methods,
    reason = "a ready boot is the last use the sweep ages the venv by"
)]
pub(crate) fn restart_sweep_clock(venv: &Path) {
    let now = std::time::SystemTime::now();
    let _best_effort = std::fs::File::open(venv).and_then(|dir| dir.set_modified(now));
}

fn settled(venv: &Path) -> bool {
    std::fs::metadata(venv)
        .and_then(|meta| meta.modified())
        .is_ok_and(|at| {
            at.elapsed()
                .is_ok_and(|age| age.as_secs() > SWEEP_AFTER_SECS)
        })
}

fn sweep(current: &Path, owned: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Some(entries) = current
        .parent()
        .and_then(|parent| std::fs::read_dir(parent).ok())
    else {
        return Vec::new();
    };
    let stale: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            path.as_path() != current
                && settled(path)
                && name.is_some_and(|name| owned(&name) && !name.ends_with(BOOTSTRAP_LOCK_NAME))
        })
        .collect();
    if stale.is_empty() {
        return stale;
    }
    let Some(running) = running_commands() else {
        return Vec::new();
    };
    stale
        .into_iter()
        .filter(|venv| !running.contains(&format!("{}/", venv.display())))
        .filter(|venv| {
            try_bootstrap_lock(venv).is_some_and(|_held| std::fs::remove_dir_all(venv).is_ok())
        })
        .collect()
}

pub fn remove_stale_venvs(current: &Path) -> Vec<PathBuf> {
    // Incident: the pre-0.111.0 unhashed `kernel-venv` matched no prefix and stayed.
    sweep(current, |name| {
        name == "kernel-venv" || name.starts_with("kernel-venv-")
    })
}

pub(crate) fn remove_stale_overlays(current: &Path) -> Vec<PathBuf> {
    sweep(current, |name| name.starts_with(OVERLAY_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dependency_venv_is_named_by_pins_extras_and_dependency_list() {
        let lists = |deps: &str| vec![format!(" = [{deps}")];
        assert_ne!(
            venv_name("2\0ipykernel>=7,<8\0dill", &[], &[]),
            venv_name("3\0ipykernel>=7,<8\0dill", &[], &[]),
            "a bumped schema or pin must build a new venv, never rebuild the live one in place"
        );
        assert_ne!(
            venv_name("check", &["pandas"], &[]),
            venv_name("check", &["pandas", "firecrawl-anydoc"], &[]),
            "two commits whose extras differ must not share one venv"
        );
        assert_ne!(
            venv_name("check", &[], &lists("\"tyro\"")),
            venv_name("check", &[], &lists("")),
            "a pyproject dependency change must build a new dependency venv"
        );
    }

    #[test]
    fn a_pyproject_names_its_dependency_array_not_its_version() {
        let before = "[project]\nversion = \"0.1.0\"\ndependencies = [\n  \"nest-asyncio\",\n]\n";
        let after = before.replace("0.1.0", "0.2.0");
        assert_eq!(dependency_list(before), dependency_list(&after));
        assert!(dependency_list(before).contains("nest-asyncio"));
    }

    #[test]
    fn each_runtime_identity_gets_its_own_overlay_inside_the_dependency_venv() {
        let deps = Path::new("/nonexistent/kernel-venv-aaaa0000");
        let x = overlay_dir(deps, "sha256:0123456789abcdef");
        assert_eq!(x, deps.join("overlays").join("rt-0123456789ab"));
        assert_ne!(x, overlay_dir(deps, "sha256:fedcba9876543210"));
    }
}
