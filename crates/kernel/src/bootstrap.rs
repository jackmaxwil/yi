use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use yi_types::kernel::BootstrapVersion;

pub const BOOTSTRAP_SCHEMA: u64 = 1;
const PYTHON_VERSION: &str = "3.11";
const IPYKERNEL_REQUIREMENT: &str = "ipykernel";
const STATE_SNAPSHOT_REQUIREMENT: &str = "dill";
pub const DEFAULT_RLM_EXTRA_UV_ARGS: [&str; 12] = [
    "requests",
    "httpx",
    "pyyaml",
    "tomli",
    "python-dotenv",
    "pandas",
    "numpy",
    "scipy",
    "beautifulsoup4",
    "lxml",
    "pydantic",
    "tyro",
];
const DEFAULT_RLM_EXTRA_IMPORT_NAMES: [&str; 12] = [
    "requests", "httpx", "yaml", "tomli", "dotenv", "pandas", "numpy", "scipy", "bs4", "lxml",
    "pydantic", "tyro",
];
const UV_INSTALL_COMMAND: &str = "curl -LsSf https://astral.sh/uv/install.sh | sh";
pub const RUNTIME_READY_CHECK: &str = "import inspect; import rlm; from rlm import McpIntegration; import rlm.mcp as mcp; from rlm.harness import HarnessEntry; _harness_methods = [\"create_memory\",\"update_memory\",\"delete_memory\",\"create_skill\",\"update_skill\",\"delete_skill\",\"create_subagent\",\"update_subagent\",\"delete_subagent\",\"create_prompt_note\",\"update_prompt_note\",\"delete_prompt_note\",\"record_refinement\"]; assert callable(mcp.list_tools); assert callable(mcp.call_tool); assert hasattr(rlm, 'run'); assert callable(rlm); assert hasattr(rlm, 'rlm'); assert callable(rlm.rlm); assert callable(rlm.host_request); assert callable(rlm.find_models); assert callable(rlm.rlm.find_models); assert hasattr(rlm, 'harness'); assert hasattr(rlm, 'get_harness_state'); assert hasattr(rlm.rlm, 'harness'); assert hasattr(rlm.rlm, 'get_harness_state'); assert all(callable(getattr(_harness, _method, None)) for _harness in (rlm.harness, rlm.rlm.harness) for _method in _harness_methods); assert 'reference' in HarnessEntry.__dataclass_fields__; assert 'scope' in HarnessEntry.__dataclass_fields__; assert 'reference' in inspect.signature(rlm.harness.create_skill).parameters; assert 'reference' in inspect.signature(rlm.harness.update_skill).parameters; assert 'global_' in inspect.signature(rlm.harness.create_memory).parameters; assert 'global_' in inspect.signature(rlm.get_harness_state).parameters; assert not hasattr(rlm, 'background'); assert not hasattr(rlm.rlm, 'background')";
const BOOTSTRAP_VERSION_FILE: &str = ".bootstrap-version";
const BOOTSTRAP_LOCK_NAME: &str = ".bootstrap.lock";
const BOOTSTRAP_LOCK_RETRY_MS: u64 = 100;
const BOOTSTRAP_LOCK_STALE_WITHOUT_PID_MS: u128 = 30_000;

pub type ProgressFn = dyn Fn(&str) + Send + Sync;

pub struct BootstrapOptions {
    pub on_progress: Option<Box<ProgressFn>>,
    pub home: PathBuf,
    pub runtime_source_dir: PathBuf,
    pub skills_source_dir: PathBuf,
}

impl BootstrapOptions {
    fn progress(&self, message: &str) {
        match &self.on_progress {
            Some(callback) => callback(message),
            None => eprintln!("{message}"),
        }
    }
}

pub fn default_runtime_source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("python")
        .join("yi_runtime")
}

/// Bundled Python skills installed into the kernel venv (design §3):
/// (import name, directory under python/skills). Install order is declared
/// order — the dependency toposort is excised until a skill grows a sibling
/// dep.
pub const PYTHON_SKILLS: [(&str, &str); 3] = [
    ("compact", "compact"),
    ("attach_image", "attach-image"),
    ("goal", "goal"),
];

pub fn default_skills_source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("python")
        .join("skills")
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn kernel_venv_dir(home: &Path) -> PathBuf {
    match env_path("YI_KERNEL_VENV") {
        Some(dir) => dir,
        None => home.join(".yi").join("kernel-venv"),
    }
}

fn xdg_kernel_venv_dir(home: &Path) -> PathBuf {
    let data_home = match env_path("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => home.join(".local").join("share"),
    };
    data_home.join("yi").join("kernel-venv")
}

fn resolve_writable_venv_dir(home: &Path) -> Result<PathBuf, String> {
    let primary = kernel_venv_dir(home);
    let parent = primary.parent().unwrap_or(&primary);
    match std::fs::create_dir_all(parent) {
        Ok(()) => Ok(primary),
        Err(primary_error) => {
            if env_path("YI_KERNEL_VENV").is_some() {
                return Err(format!(
                    "couldn't create kernel venv parent directory for {}: {primary_error}",
                    primary.display()
                ));
            }
            let fallback = xdg_kernel_venv_dir(home);
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

#[expect(
    clippy::disallowed_methods,
    reason = "bootstrap owns its subprocess probes and uv runs (design K1)"
)]
fn command(program: &Path) -> std::process::Command {
    std::process::Command::new(program)
}

fn run(program: &Path, args: &[&str], inherit: bool) -> Result<(), String> {
    let mut cmd = command(program);
    cmd.args(args);
    if inherit {
        cmd.stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
    } else {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    cmd.stdin(std::process::Stdio::null());
    let status = cmd
        .status()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {} failed with {status}",
            program.display(),
            args.join(" ")
        ))
    }
}

fn python_imports(python: &Path, module: &str) -> bool {
    run(python, &["-c", &format!("import {module}")], false).is_ok()
}

pub fn has_runtime(python: &Path) -> bool {
    run(python, &["-c", RUNTIME_READY_CHECK], false).is_ok()
}

fn missing_extra_imports(python: &Path) -> Vec<String> {
    DEFAULT_RLM_EXTRA_IMPORT_NAMES
        .iter()
        .filter(|name| !python_imports(python, name))
        .map(|name| (*name).to_owned())
        .collect()
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path_value = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_value) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn ensure_uv(options: &BootstrapOptions) -> Result<PathBuf, String> {
    if let Some(uv) = find_executable("uv") {
        return Ok(uv);
    }
    let local_uv = options.home.join(".local").join("bin").join("uv");
    if is_executable(&local_uv) {
        return Ok(local_uv);
    }
    if std::env::var_os("YI_INSTALL_UV").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Err(format!(
            "uv is required to set up the Python kernel. Install uv yourself: {UV_INSTALL_COMMAND}, or set YI_INSTALL_UV=1 to let yi run that installer."
        ));
    }
    options.progress("› installing uv (one-time)…");
    run(Path::new("sh"), &["-c", UV_INSTALL_COMMAND], true).map_err(|error| {
        format!(
            "couldn't install uv from astral.sh; install it yourself: {UV_INSTALL_COMMAND}, then re-run yi. {error}"
        )
    })?;
    if is_executable(&local_uv) {
        return Ok(local_uv);
    }
    find_executable("uv")
        .ok_or_else(|| "uv install completed but binary not found at ~/.local/bin/uv".to_owned())
}

pub fn resolve_runtime_identity(source_dir: &Path) -> Result<String, String> {
    resolve_python_identity(source_dir, None)
}

/// Content hash of the runtime package plus (when present) the bundled skills
/// tree: any Python change invalidates the venv. A failure here must surface
/// rather than fall back to a static identity — recording one would
/// permanently mask later source changes.
pub fn resolve_python_identity(
    source_dir: &Path,
    skills_dir: Option<&Path>,
) -> Result<String, String> {
    let mut files = vec![source_dir.join("pyproject.toml")];
    collect_py_files(&source_dir.join("src").join("rlm"), &mut files)?;
    if let Some(skills_dir) = skills_dir {
        for (_, subdir) in PYTHON_SKILLS {
            let skill_dir = skills_dir.join(subdir);
            if skill_dir.is_dir() {
                files.push(skill_dir.join("pyproject.toml"));
                collect_py_files(&skill_dir.join("src"), &mut files)?;
            }
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for file in &files {
        let relative = file
            .strip_prefix(source_dir)
            .ok()
            .or_else(|| skills_dir.and_then(|dir| file.strip_prefix(dir).ok()))
            .unwrap_or(file);
        hash.update(relative.to_string_lossy().as_bytes());
        hash.update(b"\0");
        let contents =
            std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
        hash.update(&contents);
        hash.update(b"\0");
    }
    let digest = hash.finalize();
    let mut hex = String::with_capacity(70);
    hex.push_str("sha256:");
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

fn collect_py_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_py_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "py") {
            files.push(path);
        }
    }
    Ok(())
}

fn read_bootstrap_version(venv: &Path) -> Option<BootstrapVersion> {
    let text = std::fs::read_to_string(venv.join(BOOTSTRAP_VERSION_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn expected_skills() -> Vec<String> {
    PYTHON_SKILLS
        .iter()
        .map(|(import_name, _)| (*import_name).to_owned())
        .collect()
}

fn bootstrap_version_current(version: Option<&BootstrapVersion>, runtime_identity: &str) -> bool {
    version.is_some_and(|version| {
        version.schema == BOOTSTRAP_SCHEMA
            && version.ipykernel == IPYKERNEL_REQUIREMENT
            && version.runtime == runtime_identity
            && version.extra_args == DEFAULT_RLM_EXTRA_UV_ARGS
            && version.skills == expected_skills()
            && version.snapshot.as_deref() == Some(STATE_SNAPSHOT_REQUIREMENT)
    })
}

fn write_bootstrap_version(venv: &Path, runtime_identity: &str) -> Result<(), String> {
    let version = BootstrapVersion {
        schema: BOOTSTRAP_SCHEMA,
        ipykernel: IPYKERNEL_REQUIREMENT.to_owned(),
        runtime: runtime_identity.to_owned(),
        extra_args: DEFAULT_RLM_EXTRA_UV_ARGS
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect(),
        skills: expected_skills(),
        snapshot: Some(STATE_SNAPSHOT_REQUIREMENT.to_owned()),
        extra: serde_json::Map::new(),
    };
    let text = serde_json::to_string(&version).map_err(|error| error.to_string())?;
    std::fs::write(venv.join(BOOTSTRAP_VERSION_FILE), format!("{text}\n"))
        .map_err(|error| error.to_string())
}

fn bootstrap_lock_dir(venv: &Path) -> PathBuf {
    let name = venv
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    venv.with_file_name(format!("{name}{BOOTSTRAP_LOCK_NAME}"))
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    // kill -0 probes liveness; EPERM still means the pid exists.
    command(Path::new("kill"))
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    true
}

fn read_lock_pid(lock_dir: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(lock_dir.join("pid")).ok()?;
    let pid: u32 = raw.trim().parse().ok()?;
    (pid > 0).then_some(pid)
}

fn lock_missing_pid_is_stale(lock_dir: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_dir) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age.as_millis() > BOOTSTRAP_LOCK_STALE_WITHOUT_PID_MS)
        .unwrap_or(false)
}

struct BootstrapLock {
    dir: PathBuf,
}

impl Drop for BootstrapLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn acquire_bootstrap_lock(venv: &Path) -> Result<BootstrapLock, String> {
    let lock_dir = bootstrap_lock_dir(venv);
    if let Some(parent) = lock_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => {
                let _ = std::fs::write(lock_dir.join("pid"), format!("{}\n", std::process::id()));
                return Ok(BootstrapLock { dir: lock_dir });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = match read_lock_pid(&lock_dir) {
                    Some(pid) => !process_is_running(pid),
                    None => lock_missing_pid_is_stale(&lock_dir),
                };
                if stale {
                    let _ = std::fs::remove_dir_all(&lock_dir);
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(BOOTSTRAP_LOCK_RETRY_MS));
            }
            Err(error) => return Err(format!("{}: {error}", lock_dir.display())),
        }
    }
}

fn kernel_ready(python: &Path, venv: &Path, runtime_identity: &str) -> bool {
    bootstrap_version_current(read_bootstrap_version(venv).as_ref(), runtime_identity)
        && python_imports(python, "ipykernel")
        && has_runtime(python)
}

fn bootstrap_venv(
    venv: &Path,
    options: &BootstrapOptions,
    runtime_identity: &str,
) -> Result<(), String> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let uv = ensure_uv(options)?;
    let python = venv.join("bin").join("python");
    let venv_text = venv.to_string_lossy().into_owned();
    let python_text = python.to_string_lossy().into_owned();
    let runtime_dir = options.runtime_source_dir.to_string_lossy().into_owned();
    run(&uv, &["python", "install", PYTHON_VERSION], false)?;
    run(
        &uv,
        &["venv", &venv_text, "--python", PYTHON_VERSION, "--seed"],
        false,
    )?;
    let mut install = vec![
        "pip",
        "install",
        "--python",
        &python_text,
        IPYKERNEL_REQUIREMENT,
        STATE_SNAPSHOT_REQUIREMENT,
        &runtime_dir,
    ];
    install.extend(DEFAULT_RLM_EXTRA_UV_ARGS);
    run(&uv, &install, false)?;
    for (import_name, subdir) in PYTHON_SKILLS {
        let skill_dir = options.skills_source_dir.join(subdir);
        if !skill_dir.is_dir() {
            options.progress(&format!(
                "Warning: Python skill {import_name} source missing at {}; skipping",
                skill_dir.display()
            ));
            continue;
        }
        let skill_text = skill_dir.to_string_lossy().into_owned();
        if let Err(error) = run(
            &uv,
            &["pip", "install", "--python", &python_text, &skill_text],
            false,
        ) {
            // A broken skill degrades to its unavailable wrapper in the
            // bootstrap cell; it must never fail the whole venv.
            options.progress(&format!(
                "Warning: Python skill {import_name} failed to install and will be unavailable: {error}"
            ));
        }
    }
    write_bootstrap_version(venv, runtime_identity)
}

pub fn kernel_python(venv: &Path) -> PathBuf {
    venv.join("bin").join("python")
}

pub fn ensure_kernel_python(options: &BootstrapOptions) -> Result<PathBuf, String> {
    if let Some(python) = env_path("YI_KERNEL_PYTHON") {
        let mut missing = Vec::new();
        if !python_imports(&python, "ipykernel") {
            missing.push("ipykernel".to_owned());
        }
        if !has_runtime(&python) {
            missing.push(
                "a current yi-runtime with callable rlm.run, rlm.host_request, and explicit harness CRUD methods".to_owned(),
            );
        }
        if missing.is_empty() {
            let missing_extras = missing_extra_imports(&python);
            if !missing_extras.is_empty() {
                missing.push(format!(
                    "default Python packages ({})",
                    missing_extras.join(", ")
                ));
            }
        }
        if missing.is_empty() {
            return Ok(python);
        }
        return Err(format!(
            "YI_KERNEL_PYTHON points to a Python missing {}: {}",
            missing.join(" and "),
            python.display()
        ));
    }

    let venv = resolve_writable_venv_dir(&options.home)?;
    let python = venv.join("bin").join("python");
    let runtime_identity = resolve_python_identity(
        &options.runtime_source_dir,
        Some(&options.skills_source_dir),
    )?;
    if kernel_ready(&python, &venv, &runtime_identity) {
        return Ok(python);
    }

    let lock = acquire_bootstrap_lock(&venv)?;
    let result = (|| {
        if kernel_ready(&python, &venv, &runtime_identity) {
            return Ok(python.clone());
        }
        options.progress("› setting up python kernel (one-time, ~30s)…");
        if venv.exists() {
            options.progress("rebuilding kernel venv");
            std::fs::remove_dir_all(&venv).map_err(|error| error.to_string())?;
        }
        bootstrap_venv(&venv, options, &runtime_identity)?;
        options.progress("✓ ready");
        Ok(python.clone())
    })();
    drop(lock);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(runtime: &str) -> BootstrapVersion {
        BootstrapVersion {
            schema: BOOTSTRAP_SCHEMA,
            ipykernel: IPYKERNEL_REQUIREMENT.to_owned(),
            runtime: runtime.to_owned(),
            extra_args: DEFAULT_RLM_EXTRA_UV_ARGS
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
            skills: expected_skills(),
            snapshot: Some(STATE_SNAPSHOT_REQUIREMENT.to_owned()),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn any_bootstrap_version_mismatch_forces_rebuild() {
        let current = version("sha256:abc");
        assert!(bootstrap_version_current(Some(&current), "sha256:abc"));
        assert!(!bootstrap_version_current(Some(&current), "sha256:other"));
        let mut wrong_schema = version("sha256:abc");
        wrong_schema.schema = BOOTSTRAP_SCHEMA.wrapping_add(1);
        assert!(!bootstrap_version_current(
            Some(&wrong_schema),
            "sha256:abc"
        ));
        let mut wrong_extras = version("sha256:abc");
        wrong_extras.extra_args.pop();
        assert!(!bootstrap_version_current(
            Some(&wrong_extras),
            "sha256:abc"
        ));
        let mut no_snapshot = version("sha256:abc");
        no_snapshot.snapshot = None;
        assert!(
            !bootstrap_version_current(Some(&no_snapshot), "sha256:abc"),
            "a pre-dill venv must rebuild once"
        );
        assert!(!bootstrap_version_current(None, "sha256:abc"));
    }

    #[test]
    fn dead_pid_lock_is_broken_live_pid_lock_holds() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("yi-kernel-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let venv = root.join("kernel-venv");
        let lock_dir = bootstrap_lock_dir(&venv);
        std::fs::create_dir_all(&lock_dir).map_err(|error| error.to_string())?;
        std::fs::write(lock_dir.join("pid"), "4194303\n").map_err(|error| error.to_string())?;
        let lock = acquire_bootstrap_lock(&venv)?;
        let held =
            std::fs::read_to_string(lock_dir.join("pid")).map_err(|error| error.to_string())?;
        assert_eq!(held.trim(), std::process::id().to_string());
        drop(lock);
        assert!(!lock_dir.exists(), "drop must release the lock directory");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn runtime_identity_tracks_python_source_changes() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("yi-kernel-ident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rlm = root.join("src").join("rlm");
        std::fs::create_dir_all(&rlm).map_err(|error| error.to_string())?;
        std::fs::write(root.join("pyproject.toml"), "[project]\n")
            .map_err(|error| error.to_string())?;
        std::fs::write(rlm.join("__init__.py"), "x = 1\n").map_err(|error| error.to_string())?;
        let before = resolve_runtime_identity(&root)?;
        std::fs::write(rlm.join("__init__.py"), "x = 2\n").map_err(|error| error.to_string())?;
        let after = resolve_runtime_identity(&root)?;
        assert_ne!(
            before, after,
            "a runtime source edit must invalidate the venv"
        );
        assert!(before.starts_with("sha256:"));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
