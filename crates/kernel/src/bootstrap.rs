use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use yi_types::kernel::BootstrapVersion;

pub(crate) use crate::lock::acquire_bootstrap_lock;
pub use crate::lock::{
    lock_is_stale, lock_missing_pid_is_stale, process_is_running, read_lock_pid,
};

pub const BOOTSTRAP_SCHEMA: u64 = 1;
const PYTHON_VERSION: &str = "3.11";
const IPYKERNEL_REQUIREMENT: &str = "ipykernel";
const STATE_SNAPSHOT_REQUIREMENT: &str = "dill";
pub const DEFAULT_RLM_EXTRA_UV_ARGS: [&str; 15] = [
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
    "firecrawl-anydoc>=0.2.4,<0.3",
    "pdf-inspector>=1.19,<2",
    "openpyxl",
];
const DEFAULT_RLM_EXTRA_IMPORT_NAMES: [&str; 15] = [
    "requests",
    "httpx",
    "yaml",
    "tomli",
    "dotenv",
    "pandas",
    "numpy",
    "scipy",
    "bs4",
    "lxml",
    "pydantic",
    "tyro",
    "anydoc",
    "pdf_inspector",
    "openpyxl",
];
/// csv is left out because read shows it as the text it is; the alias candidates are checked live.
const DOCUMENT_FORMATS_PROBE: &str = r#"import json, typing, anydoc
kinds = [kind for kind in typing.get_args(anydoc.Format) if kind != "csv"]
aliases = {}
for ext in ("docm", "dot", "dotx", "dotm", "xls", "xlsm", "xlsb", "xlt", "xltx", "xltm", "pps", "ppsx", "ppsm", "pot", "potx", "potm", "pptm", "fodt", "fods", "fodp"):
    kind = anydoc.format_from_extension(ext)
    if kind in kinds and ext != kind:
        aliases.setdefault(kind, []).append(ext)
print(json.dumps([kind + (f" ({', '.join(aliases[kind])})" if kind in aliases else "") for kind in kinds]))"#;
/// The converter wheels are the only extras a `YI_KERNEL_PYTHON` may lack: without them read
/// shows a document as bytes and nothing else changes.
const OPTIONAL_IMPORT_NAMES: [&str; 2] = ["anydoc", "pdf_inspector"];
const DOCUMENT_FORMATS_KEY: &str = "documentFormats";
/// The probe's own hash rides in the record, so a reworded probe re-asks the wheel on the next
/// readiness check instead of serving a list the code no longer produces (no rebuild needed).
const DOCUMENT_PROBE_KEY: &str = "documentProbe";

fn document_probe_hash() -> String {
    format!("{:016x}", fnv1a(DOCUMENT_FORMATS_PROBE.as_bytes()))
}
const UV_INSTALL_COMMAND: &str = "curl -LsSf https://astral.sh/uv/install.sh | sh";
pub const RUNTIME_READY_CHECK: &str = "import inspect; import rlm; import rlm.mcp as mcp; from rlm.harness import HarnessEntry; _harness_methods = [\"create_memory\",\"update_memory\",\"delete_memory\",\"create_skill\",\"update_skill\",\"delete_skill\",\"create_subagent\",\"update_subagent\",\"delete_subagent\",\"create_prompt_note\",\"update_prompt_note\",\"delete_prompt_note\",\"record_refinement\"]; assert callable(mcp.list_tools); assert callable(mcp.call_tool); assert callable(rlm); assert all(hasattr(rlm, _name) for _name in rlm.__all__); assert not hasattr(rlm, 'rlm'); assert all(callable(getattr(rlm.harness, _method, None)) for _method in _harness_methods); assert 'reference' in HarnessEntry.__dataclass_fields__; assert 'scope' in HarnessEntry.__dataclass_fields__; assert 'reference' in inspect.signature(rlm.harness.create_skill).parameters; assert 'reference' in inspect.signature(rlm.harness.update_skill).parameters; assert 'global_' in inspect.signature(rlm.harness.create_memory).parameters; assert 'global_' in inspect.signature(rlm.get_harness_state).parameters; assert not inspect.iscoroutinefunction(rlm.bash); assert hasattr(rlm.BashHandle, '__await__'); assert not hasattr(rlm, 'background'); from pathlib import Path as _P; assert rlm.RLMSubagent(rlm_child_id='c', active_session_id=None, session_id=None, session_name='kid', session_dir=_P('.'), status='idle').name == 'kid'";
const BOOTSTRAP_VERSION_FILE: &str = ".bootstrap-version";
/// `python/yi_runtime` and `python/skills`, deflated by `build.rs`.
const PYTHON_EMBED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/python.zz"));
const PYTHON_STAMP_FILE: &str = ".stamp";

pub type ProgressFn = dyn Fn(&str) + Send + Sync;

pub struct BootstrapOptions {
    pub on_progress: Option<Box<ProgressFn>>,
    pub home: PathBuf,
    pub runtime_source_dir: PathBuf,
    pub skills_source_dir: PathBuf,
    /// `None` discovers the toolchain; a test names one to build without `uv`.
    pub toolchain: Option<Toolchain>,
    /// `None` keys the venv under `home` (or `YI_KERNEL_VENV`); a test isolates one.
    pub venv_dir: Option<PathBuf>,
}

/// What builds the venv: `uv` when present, else the machine's own python3 3.11+.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toolchain {
    Uv(PathBuf),
    System(PathBuf),
}

impl Toolchain {
    pub fn describe(&self) -> String {
        match self {
            Self::Uv(uv) => format!("uv {}", uv.display()),
            Self::System(python) => {
                let version = output(python, &["-c", PYTHON_VERSION_PRINT]).unwrap_or_default();
                format!("python3 {} {} (no uv)", version.trim(), python.display())
            }
        }
    }
}

const SYSTEM_PYTHON_CHECK: &str = "import sys, venv, ensurepip; assert sys.version_info >= (3, 11)";
const PYTHON_VERSION_PRINT: &str = "import sys; print('%d.%d' % sys.version_info[:2])";
const SYSTEM_PYTHONS: [&str; 4] = ["python3", "python3.13", "python3.12", "python3.11"];

impl BootstrapOptions {
    fn progress(&self, message: &str) {
        match &self.on_progress {
            Some(callback) => callback(message),
            None => eprintln!("{message}"),
        }
    }
}

fn has_python_sources(root: &Path) -> bool {
    root.join("yi_runtime").is_dir()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Length plus hash of the deflated archive: a rebuilt runtime changes it and re-unpacks.
fn embed_stamp() -> String {
    format!("{}:{:016x}", PYTHON_EMBED.len(), fnv1a(PYTHON_EMBED))
}

fn home_tree_current(root: &Path) -> bool {
    has_python_sources(root)
        && std::fs::read_to_string(root.join(PYTHON_STAMP_FILE))
            .is_ok_and(|stamp| stamp.trim() == embed_stamp())
}

fn take(bytes: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let (len, rest) = bytes
        .split_first_chunk::<4>()
        .ok_or("truncated python archive")?;
    let len = usize::try_from(u32::from_le_bytes(*len)).map_err(|error| error.to_string())?;
    rest.split_at_checked(len)
        .ok_or_else(|| "truncated python archive".to_owned())
}

fn unpack_entries(mut rest: &[u8], into: &Path) -> Result<(), String> {
    while !rest.is_empty() {
        let (path, after) = take(rest)?;
        let (content, after) = take(after)?;
        rest = after;
        let relative = std::str::from_utf8(path).map_err(|error| error.to_string())?;
        let relative = Path::new(relative);
        if !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "python archive entry escapes its root: {relative:?}"
            ));
        }
        let target = into.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&target, content)
            .map_err(|error| format!("{}: {error}", target.display()))?;
    }
    Ok(())
}

/// The old tree is moved aside, never written into, so a symlink there is refused rather
/// than followed and a reader sees the previous tree or the whole new one.
fn replace_dir(staging: &Path, target: &Path) -> Result<(), String> {
    let describe = |error: std::io::Error| format!("{}: {error}", target.display());
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!(
                "{} is a symlink; not replacing it",
                target.display()
            ));
        }
        Ok(_) => {
            let old = target.with_extension(format!("old-{}", std::process::id()));
            std::fs::rename(target, &old).map_err(describe)?;
            let _ = std::fs::remove_dir_all(&old);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(describe(error)),
    }
    std::fs::rename(staging, target).map_err(describe)
}

/// Writes the embedded runtime to `<home>/.yi/python`, whole or not at all: a sibling temp
/// dir is filled and stamped, then renamed over whatever was there.
pub fn unpack_embedded_python(home: &Path) -> Result<PathBuf, String> {
    let data = miniz_oxide::inflate::decompress_to_vec_zlib(PYTHON_EMBED)
        .map_err(|error| format!("embedded python archive: {error}"))?;
    let parent = home.join(".yi");
    std::fs::create_dir_all(&parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let target = parent.join("python");
    let staging = parent.join(format!("python.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = unpack_entries(&data, &staging)
        .and_then(|()| {
            std::fs::write(staging.join(PYTHON_STAMP_FILE), embed_stamp())
                .map_err(|error| format!("{}: {error}", staging.display()))
        })
        .and_then(|()| replace_dir(&staging, &target));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map(|()| target)
}

/// Directory holding `yi_runtime` and `skills`: `python/` in the tree above the exe, then
/// `~/.yi/python` unpacked from the embed, then the compile-time path (the build machine's).
fn resolve_python_root(exe: Option<&Path>, home: Option<&Path>, fallback: PathBuf) -> PathBuf {
    let mut dir = exe.and_then(Path::parent);
    // deps/ -> debug/ -> target/ -> root is three; the spare levels cost a
    // stat each and keep nested target dirs (worktrees, `--target`) working.
    for _ in 0..6 {
        let Some(candidate) = dir else { break };
        let python = candidate.join("python");
        if has_python_sources(&python) {
            return python;
        }
        dir = candidate.parent();
    }
    let Some(home) = home else { return fallback };
    let root = home.join(".yi").join("python");
    if home_tree_current(&root) {
        return root;
    }
    match unpack_embedded_python(home) {
        Ok(root) => root,
        Err(error) => {
            eprintln!(
                "warning: python runtime not unpacked under {}: {error}",
                home.display()
            );
            fallback
        }
    }
}

pub fn python_root() -> PathBuf {
    resolve_python_root(
        std::env::current_exe().ok().as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("python"),
    )
}

pub fn default_runtime_source_dir() -> PathBuf {
    python_root().join("yi_runtime")
}

/// (import name, directory under python/skills). Install order is declared
/// order; the dependency toposort waits until a skill grows a sibling dep.
pub const PYTHON_SKILLS: [(&str, &str); 4] = [
    ("compact", "compact"),
    ("attach_image", "attach-image"),
    ("goal", "goal"),
    ("memory", "memory"),
];

pub fn default_skills_source_dir() -> PathBuf {
    python_root().join("skills")
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Incident: one `~/.yi/kernel-venv` served every commit, so two sessions whose ready checks
/// differed rebuilt it back and forth. The check and the extras the venv carries name it now.
fn venv_name(check: &str, extras: &[&str]) -> String {
    let mut hash = Sha256::new();
    hash.update(check.as_bytes());
    for extra in extras {
        hash.update(b"\0");
        hash.update(extra.as_bytes());
    }
    let digest = hash.finalize();
    let slot: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("kernel-venv-{slot}")
}

fn default_kernel_venv_dir(home: &Path, check: &str) -> PathBuf {
    home.join(".yi")
        .join(venv_name(check, &DEFAULT_RLM_EXTRA_UV_ARGS))
}

pub fn kernel_venv_dir(home: &Path) -> PathBuf {
    match env_path("YI_KERNEL_VENV") {
        Some(dir) => dir,
        None => default_kernel_venv_dir(home, RUNTIME_READY_CHECK),
    }
}

fn xdg_kernel_venv_dir(home: &Path) -> PathBuf {
    let data_home = match env_path("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => home.join(".local").join("share"),
    };
    data_home
        .join("yi")
        .join(venv_name(RUNTIME_READY_CHECK, &DEFAULT_RLM_EXTRA_UV_ARGS))
}

fn resolve_writable_venv_dir(options: &BootstrapOptions) -> Result<PathBuf, String> {
    let home = &options.home;
    let primary = options
        .venv_dir
        .clone()
        .unwrap_or_else(|| kernel_venv_dir(home));
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
pub(crate) fn command(program: &Path) -> std::process::Command {
    std::process::Command::new(program)
}

fn output(program: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = command(program);
    cmd.args(args).stdin(std::process::Stdio::null());
    let out = cmd
        .output()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Err(format!(
            "{} {} failed with {}: {}",
            program.display(),
            args.join(" "),
            out.status,
            tail.trim()
        ))
    }
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
        .filter(|name| !OPTIONAL_IMPORT_NAMES.contains(name))
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

/// The first python3 on PATH that is 3.11+ and carries `venv` and `ensurepip`.
pub fn find_system_python() -> Option<PathBuf> {
    // Incident: a python whose `ensurepip` imports but cannot bootstrap pip (Debian without
    // python3-venv, a CI runner's build) failed `-m venv` halfway; the module run is the probe.
    SYSTEM_PYTHONS
        .iter()
        .filter_map(|name| find_executable(name))
        .find(|python| {
            run(python, &["-c", SYSTEM_PYTHON_CHECK], false).is_ok()
                && run(python, &["-m", "ensurepip", "--version"], false).is_ok()
        })
}

/// uv, else the machine's python3 3.11+ with venv, else the uv installer when asked for.
pub fn find_toolchain(options: &BootstrapOptions) -> Result<Toolchain, String> {
    if let Some(uv) = find_executable("uv") {
        return Ok(Toolchain::Uv(uv));
    }
    let local_uv = options.home.join(".local").join("bin").join("uv");
    if is_executable(&local_uv) {
        return Ok(Toolchain::Uv(local_uv));
    }
    if let Some(python) = find_system_python() {
        return Ok(Toolchain::System(python));
    }
    if std::env::var_os("YI_INSTALL_UV").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Err(format!(
            "no uv and no python3 3.11+ with venv on PATH. Install uv ({UV_INSTALL_COMMAND}), or python3-venv, or set YI_INSTALL_UV=1 to let yi run that installer."
        ));
    }
    options.progress("› installing uv (one-time)…");
    run(Path::new("sh"), &["-c", UV_INSTALL_COMMAND], true).map_err(|error| {
        format!(
            "couldn't install uv from astral.sh; install it yourself: {UV_INSTALL_COMMAND}, then re-run yi. {error}"
        )
    })?;
    if is_executable(&local_uv) {
        return Ok(Toolchain::Uv(local_uv));
    }
    find_executable("uv")
        .map(Toolchain::Uv)
        .ok_or_else(|| "uv install completed but binary not found at ~/.local/bin/uv".to_owned())
}

fn create_venv(toolchain: &Toolchain, venv_text: &str) -> Result<(), String> {
    match toolchain {
        Toolchain::Uv(uv) => {
            // The interpreter that exists; a managed CPython only when none does.
            let system = find_system_python();
            if system.is_none() {
                run(uv, &["python", "install", PYTHON_VERSION], false)?;
            }
            let python = system.map_or_else(
                || PYTHON_VERSION.to_owned(),
                |path| path.to_string_lossy().into_owned(),
            );
            output(
                uv,
                &[
                    "venv",
                    venv_text,
                    "--python",
                    &python,
                    "--system-site-packages",
                ],
            )
            .map(drop)
        }
        Toolchain::System(python) => {
            output(python, &["-m", "venv", "--system-site-packages", venv_text]).map(drop)
        }
    }
}

fn pip_install(toolchain: &Toolchain, python: &Path, packages: &[&str]) -> Result<(), String> {
    let python_text = python.to_string_lossy().into_owned();
    let mut args: Vec<&str> = match toolchain {
        Toolchain::Uv(_) => vec![
            "pip",
            "install",
            "--python",
            &python_text,
            "--compile-bytecode",
        ],
        Toolchain::System(_) => vec!["-m", "pip", "install", "--quiet"],
    };
    args.extend(packages);
    match toolchain {
        Toolchain::Uv(uv) => output(uv, &args).map(drop),
        Toolchain::System(_) => output(python, &args).map(drop),
    }
}

fn missing_extra_packages(python: &Path) -> Vec<&'static str> {
    DEFAULT_RLM_EXTRA_UV_ARGS
        .iter()
        .zip(DEFAULT_RLM_EXTRA_IMPORT_NAMES.iter())
        .filter(|(_, import_name)| !python_imports(python, import_name))
        .map(|(package, _)| *package)
        .collect()
}

pub fn resolve_runtime_identity(source_dir: &Path) -> Result<String, String> {
    resolve_python_identity(source_dir, None)
}

/// Any Python change invalidates the venv. A failure here surfaces rather than falling back
/// to a static identity, which would permanently mask later source changes.
pub fn resolve_python_identity(
    source_dir: &Path,
    skills_dir: Option<&Path>,
) -> Result<String, String> {
    let mut files = vec![source_dir.join("pyproject.toml")];
    collect_py_files(&source_dir.join("src"), &mut files)?;
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

fn document_formats_of(python: &Path) -> Vec<String> {
    output(python, &["-c", DOCUMENT_FORMATS_PROBE])
        .ok()
        .and_then(|text| serde_json::from_str(text.trim()).ok())
        .unwrap_or_default()
}

pub fn document_converter(home: &Path) -> (PathBuf, Vec<String>) {
    if let Some(python) = env_path("YI_KERNEL_PYTHON") {
        let formats = document_formats_of(&python);
        return (python, formats);
    }
    let primary = kernel_venv_dir(home);
    let venv = [primary.clone(), xdg_kernel_venv_dir(home)]
        .into_iter()
        .find(|venv| kernel_python(venv).is_file())
        .unwrap_or(primary);
    let formats = read_bootstrap_version(&venv)
        .and_then(|version| version.extra.get(DOCUMENT_FORMATS_KEY).cloned())
        .and_then(|formats| serde_json::from_value(formats).ok())
        .unwrap_or_default();
    (kernel_python(&venv), formats)
}

fn write_bootstrap_version(
    venv: &Path,
    runtime_identity: &str,
    document_formats: Vec<String>,
) -> Result<(), String> {
    let mut extra = serde_json::Map::new();
    extra.insert(
        DOCUMENT_PROBE_KEY.to_owned(),
        serde_json::Value::from(document_probe_hash()),
    );
    if !document_formats.is_empty() {
        extra.insert(
            DOCUMENT_FORMATS_KEY.to_owned(),
            serde_json::Value::from(document_formats),
        );
    }
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
        extra,
    };
    let text = serde_json::to_string(&version).map_err(|error| error.to_string())?;
    std::fs::write(venv.join(BOOTSTRAP_VERSION_FILE), format!("{text}\n"))
        .map_err(|error| error.to_string())
}

fn kernel_ready(python: &Path, venv: &Path, runtime_identity: &str) -> bool {
    let version = read_bootstrap_version(venv);
    if !bootstrap_version_current(version.as_ref(), runtime_identity) {
        return false;
    }
    let live = run(
        python,
        &["-c", &format!("import ipykernel; {RUNTIME_READY_CHECK}")],
        false,
    )
    .is_ok();
    let probe = serde_json::Value::from(document_probe_hash());
    if live && version.is_some_and(|version| version.extra.get(DOCUMENT_PROBE_KEY) != Some(&probe))
    {
        let _record_refreshed =
            write_bootstrap_version(venv, runtime_identity, document_formats_of(python));
    }
    live
}

fn bootstrap_venv(
    venv: &Path,
    options: &BootstrapOptions,
    runtime_identity: &str,
) -> Result<(), String> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let toolchain = match &options.toolchain {
        Some(toolchain) => toolchain.clone(),
        None => find_toolchain(options)?,
    };
    let python = venv.join("bin").join("python");
    create_venv(&toolchain, &venv.to_string_lossy())?;
    let runtime_dir = options.runtime_source_dir.to_string_lossy().into_owned();
    pip_install(
        &toolchain,
        &python,
        &[
            IPYKERNEL_REQUIREMENT,
            STATE_SNAPSHOT_REQUIREMENT,
            &runtime_dir,
        ],
    )?;
    let skills: Vec<String> = PYTHON_SKILLS
        .iter()
        .filter_map(|(import_name, subdir)| {
            let skill_dir = options.skills_source_dir.join(subdir);
            if skill_dir.is_dir() {
                return Some(skill_dir.to_string_lossy().into_owned());
            }
            options.progress(&format!(
                "Warning: Python skill {import_name} source missing at {}; skipping",
                skill_dir.display()
            ));
            None
        })
        .collect();
    let skill_refs: Vec<&str> = skills.iter().map(String::as_str).collect();
    // A broken skill or extra degrades to its unavailable wrapper; it must never
    // fail the whole venv, and an extra the image already carries is not fetched.
    if !skill_refs.is_empty()
        && let Err(error) = pip_install(&toolchain, &python, &skill_refs)
    {
        options.progress(&format!(
            "Warning: Python skills failed to install and will be unavailable: {error}"
        ));
    }
    let extras = missing_extra_packages(&python);
    if !extras.is_empty()
        && let Err(error) = pip_install(&toolchain, &python, &extras)
    {
        options.progress(&format!(
            "Warning: default packages ({}) failed to install: {error}",
            extras.join(", ")
        ));
    }
    let document_formats = document_formats_of(&python);
    if document_formats.is_empty() && python_imports(&python, "anydoc") {
        options.progress(
            "Warning: anydoc reports no formats; read will not list documents until it does",
        );
    }
    write_bootstrap_version(venv, runtime_identity, document_formats)
}

pub fn kernel_python(venv: &Path) -> PathBuf {
    venv.join("bin").join("python")
}

/// The kernel python when nothing has to be built: `YI_KERNEL_PYTHON`, or a venv whose
/// version file and import probe both agree.
pub fn ready_kernel_python(options: &BootstrapOptions) -> Option<PathBuf> {
    if let Some(python) = env_path("YI_KERNEL_PYTHON") {
        return (python_imports(&python, "ipykernel") && has_runtime(&python)).then_some(python);
    }
    let venv = resolve_writable_venv_dir(options).ok()?;
    let python = venv.join("bin").join("python");
    let identity = resolve_python_identity(
        &options.runtime_source_dir,
        Some(&options.skills_source_dir),
    )
    .ok()?;
    kernel_ready(&python, &venv, &identity).then_some(python)
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

    let venv = resolve_writable_venv_dir(options)?;
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
    use crate::lock::bootstrap_lock_dir;
    use crate::scratch::Scratch;

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
    fn the_default_venv_is_keyed_by_the_ready_check() {
        let home = Path::new("/nonexistent/home");
        assert_ne!(
            default_kernel_venv_dir(home, "check-a"),
            default_kernel_venv_dir(home, "check-b"),
            "two commits whose ready check differs must not share one venv"
        );
        assert_eq!(
            default_kernel_venv_dir(home, RUNTIME_READY_CHECK),
            home.join(".yi")
                .join(venv_name(RUNTIME_READY_CHECK, &DEFAULT_RLM_EXTRA_UV_ARGS))
        );
        assert_ne!(
            venv_name("check", &["pandas"]),
            venv_name("check", &["pandas", "firecrawl-anydoc"]),
            "two commits whose extras differ must not share one venv"
        );
    }

    #[test]
    fn a_kernel_python_may_lack_the_converter_wheels() {
        let Some(python) = find_system_python() else {
            return;
        };
        let missing = missing_extra_imports(&python);
        assert!(
            !missing
                .iter()
                .any(|name| OPTIONAL_IMPORT_NAMES.contains(&name.as_str())),
            "the converter wheels are optional: {missing:?}"
        );
    }

    #[test]
    fn the_converter_reads_back_the_formats_its_venv_recorded() -> Result<(), String> {
        let home = Scratch::new("yi-formats").map_err(|error| error.to_string())?;
        let venv = default_kernel_venv_dir(&home, RUNTIME_READY_CHECK);
        std::fs::create_dir_all(venv.join("bin")).map_err(|error| error.to_string())?;
        std::fs::write(kernel_python(&venv), "").map_err(|error| error.to_string())?;
        let formats = vec!["docx".to_owned(), "pdf".to_owned()];
        write_bootstrap_version(&venv, "sha256:abc", formats.clone())?;
        assert_eq!(document_converter(&home), (kernel_python(&venv), formats));
        write_bootstrap_version(&venv, "sha256:abc", Vec::new())?;
        assert_eq!(
            document_converter(&home).1,
            Vec::<String>::new(),
            "a venv built without the converter claims no format"
        );
        Ok(())
    }

    #[test]
    fn dead_pid_lock_is_broken_live_pid_lock_holds() -> Result<(), String> {
        let root = Scratch::new("yi-kernel-lock").map_err(|error| error.to_string())?;
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
        Ok(())
    }

    #[test]
    fn runtime_identity_tracks_python_source_changes() -> Result<(), String> {
        let root = Scratch::new("yi-kernel-ident").map_err(|error| error.to_string())?;
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
        let yi = root.join("src").join("yi");
        std::fs::create_dir_all(&yi).map_err(|error| error.to_string())?;
        std::fs::write(yi.join("plan.py"), "y = 1\n").map_err(|error| error.to_string())?;
        assert_ne!(
            after,
            resolve_runtime_identity(&root)?,
            "every package of the wheel counts, not `rlm` alone"
        );
        Ok(())
    }

    #[test]
    fn python_sources_resolve_from_the_running_executable() {
        // The test binary lives under target/, the same place a shipped `yi`
        // sits relative to its own tree, so this fails if the walk-up ever
        // stops finding the sources it is the only mechanism for locating.
        assert!(
            default_runtime_source_dir().is_dir(),
            "runtime source not found from the executable: {}",
            default_runtime_source_dir().display()
        );
        assert!(
            default_skills_source_dir().is_dir(),
            "python skills not found from the executable: {}",
            default_skills_source_dir().display()
        );
    }

    /// Both shipped layouts, neither of which an in-repo run can distinguish
    /// from the build tree it was compiled in.
    #[test]
    fn python_root_prefers_the_unpacked_tree_then_the_installed_home() -> Result<(), String> {
        let base = Scratch::new("yi-python-root").map_err(|error| error.to_string())?;
        let tree = base.join("yi-0.0.0-target");
        let home = base.join("home");
        let sources = |root: &Path| root.join("python").join("yi_runtime");
        std::fs::create_dir_all(sources(&tree)).map_err(|error| error.to_string())?;
        let installed = home.join(".yi").join("python");
        std::fs::create_dir_all(sources(&home.join(".yi"))).map_err(|error| error.to_string())?;
        std::fs::write(installed.join(PYTHON_STAMP_FILE), embed_stamp())
            .map_err(|error| error.to_string())?;
        let fallback = base.join("build-tree");
        let exe = tree.join("bin").join("yi");

        assert_eq!(
            resolve_python_root(Some(&exe), Some(&home), fallback.clone()),
            tree.join("python"),
            "an unpacked tarball resolves through bin/yi, not through HOME"
        );
        // The installed binary sits on PATH with no tree above it.
        let on_path = home.join("bin").join("yi");
        assert_eq!(
            resolve_python_root(Some(&on_path), Some(&home), fallback.clone()),
            installed,
            "an installed binary falls back to ~/.yi/python"
        );
        assert_eq!(
            resolve_python_root(Some(&on_path), None, fallback.clone()),
            fallback,
            "with nothing to find, the compile-time path is the last resort"
        );
        Ok(())
    }

    #[test]
    fn the_embed_is_unpacked_under_home_when_no_tree_is_found() -> Result<(), String> {
        let base = Scratch::new("yi-python-embed").map_err(|error| error.to_string())?;
        let home = base.join("home");
        std::fs::create_dir_all(&home).map_err(|error| error.to_string())?;
        let exe = base.join("bin").join("yi");
        let fallback = base.join("build-tree");
        let resolve = || resolve_python_root(Some(&exe), Some(&home), fallback.clone());

        let root = resolve();
        assert_eq!(root, home.join(".yi").join("python"));
        assert!(root.join("yi_runtime").join("src").join("rlm").is_dir());
        assert!(root.join("skills").is_dir());
        assert_ne!(
            root, fallback,
            "the compile-time path never serves an installed binary"
        );
        let marker = root.join("marker");
        std::fs::write(&marker, "").map_err(|error| error.to_string())?;
        assert_eq!(resolve(), root);
        assert!(
            marker.exists(),
            "a matching stamp must leave the tree alone"
        );
        std::fs::write(root.join(PYTHON_STAMP_FILE), "stale").map_err(|error| error.to_string())?;
        assert_eq!(resolve(), root);
        assert!(!marker.exists(), "a stale stamp must re-unpack the tree");
        assert!(root.join("yi_runtime").join("pyproject.toml").is_file());
        Ok(())
    }

    #[test]
    fn the_exe_tree_wins_over_the_embed() -> Result<(), String> {
        let base = Scratch::new("yi-python-exe").map_err(|error| error.to_string())?;
        let tree = base.join("yi-target");
        let home = base.join("home");
        std::fs::create_dir_all(tree.join("python").join("yi_runtime"))
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&home).map_err(|error| error.to_string())?;
        let exe = tree.join("bin").join("yi");
        assert_eq!(
            resolve_python_root(Some(&exe), Some(&home), base.join("build-tree")),
            tree.join("python")
        );
        assert!(
            !home.join(".yi").exists(),
            "nothing is unpacked while the exe tree serves"
        );
        Ok(())
    }
}
