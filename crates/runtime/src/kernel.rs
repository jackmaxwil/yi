use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_kernel::client::{
    AbortFlag, ExecuteError, ExecuteOptions, HostFuture, HostHandlers, KernelManager, KernelOptions,
};
use yi_tools::ToolOutput;
use yi_tools::{CancelFlag, KernelBridge, KernelCellOutcome};

pub use crate::kernel_bootstrap::{RLM_BOOTSTRAP_CODE, restore_notice_text, rlm_bootstrap_code};
use crate::kernel_variables::{dump_variable_code, parse_variable_reply, read_variable_code};

pub type HostHandlerFn = dyn Fn(Map<String, Value>) -> HostFuture + Send + Sync;

/// Registered by the runtime, dispatched by yi-kernel.
#[derive(Default)]
pub struct HostRegistry {
    handlers: HashMap<String, Arc<HostHandlerFn>>,
    handles: Arc<Mutex<Vec<yi_tools::JobId>>>,
}

impl HostRegistry {
    pub fn register(
        &mut self,
        request_type: &str,
        handler: impl Fn(Map<String, Value>) -> HostFuture + Send + Sync + 'static,
    ) {
        self.handlers
            .insert(request_type.to_owned(), Arc::new(handler));
    }

    /// The host half of the kernel's `bash()` handle: five `exec.*` requests over
    /// [`yi_tools::jobs`]. A spawned job is handle-owned; only `exec.release` retires it.
    pub fn register_exec(&mut self, cwd: PathBuf) {
        let spawned = Arc::clone(&self.handles);
        self.register("exec.spawn", move |payload| {
            let cwd = cwd.clone();
            let spawned = Arc::clone(&spawned);
            Box::pin(async move {
                let command = payload
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|command| !command.trim().is_empty())
                    .ok_or_else(|| {
                        "exec.spawn requires a non-empty \"command\" argument".to_owned()
                    })?;
                let cancelled: CancelFlag = Arc::new(|| false);
                let id = yi_tools::jobs::spawn_job(command, &cwd, &cancelled, None);
                lock(&spawned).push(id);
                let mut reply = Map::new();
                reply.insert("job_id".to_owned(), Value::from(id.0));
                Ok(reply)
            })
        });
        self.register("exec.tail", |payload| {
            Box::pin(async move {
                let id = job_id_of(&payload, "exec.tail")?;
                let cursor = payload.get("cursor").and_then(Value::as_u64).unwrap_or(0);
                let chunk = yi_tools::jobs::registry()
                    .output_since(id, cursor)
                    .map_err(|error| error.to_string())?;
                let mut reply = Map::new();
                reply.insert("text".to_owned(), Value::String(chunk.text));
                reply.insert("next".to_owned(), Value::from(chunk.next));
                reply.insert("dropped".to_owned(), Value::from(chunk.dropped));
                Ok(reply)
            })
        });
        self.register("exec.poll", |payload| {
            Box::pin(async move {
                let id = job_id_of(&payload, "exec.poll")?;
                let report = yi_tools::jobs::registry()
                    .report(id)
                    .ok_or_else(|| format!("no such job: {id}"))?;
                Ok(job_report_reply(&report))
            })
        });
        self.register("exec.kill", |payload| {
            Box::pin(async move {
                let id = job_id_of(&payload, "exec.kill")?;
                let outcome = yi_tools::jobs::registry()
                    .kill(id)
                    .map_err(|error| error.to_string())?;
                let mut reply = Map::new();
                let named = match outcome {
                    yi_tools::jobs::KillOutcome::Signalled => "signalled",
                    yi_tools::jobs::KillOutcome::AlreadySettled(_) => "already_settled",
                };
                reply.insert("outcome".to_owned(), Value::String(named.to_owned()));
                Ok(reply)
            })
        });
        let released = Arc::clone(&self.handles);
        self.register("exec.release", move |payload| {
            let released = Arc::clone(&released);
            Box::pin(async move {
                let id = job_id_of(&payload, "exec.release")?;
                let report = yi_tools::jobs::registry()
                    .release(id)
                    .map_err(|error| error.to_string())?;
                lock(&released).retain(|held| *held != id);
                Ok(job_report_reply(&report))
            })
        });
    }

    /// `mcp.config` returns `{}` (Python raises its own KeyError), `mcp.refresh` throws, and
    /// `mcp.begin_login` is never registered: a 401 must not open a browser.
    pub fn register_mcp_stubs(&mut self) {
        self.register("mcp.config", |_payload| Box::pin(async { Ok(Map::new()) }));
        self.register("mcp.refresh", |_payload| {
            Box::pin(async {
                Err(
                    "mcp.refresh is unavailable: run `yi mcp login <server>` on the host"
                        .to_owned(),
                )
            })
        });
    }
}

fn job_id_of(payload: &Map<String, Value>, request: &str) -> Result<yi_tools::JobId, String> {
    payload
        .get("job_id")
        .and_then(Value::as_u64)
        .map(yi_tools::JobId)
        .ok_or_else(|| format!("{request} requires an integer \"job_id\" argument"))
}

fn job_report_reply(report: &yi_tools::JobReport) -> Map<String, Value> {
    use yi_tools::jobs::{JobState, Outcome};
    let (running, exit_code, killed) = match report.state {
        JobState::Running => (true, None, false),
        JobState::Settled(Outcome::Exited { code }) => (false, code, false),
        JobState::Settled(Outcome::Killed) => (false, None, true),
    };
    let mut reply = Map::new();
    reply.insert("job_id".to_owned(), Value::from(report.id.0));
    reply.insert("command".to_owned(), Value::String(report.command.clone()));
    reply.insert("running".to_owned(), Value::Bool(running));
    reply.insert(
        "exit_code".to_owned(),
        exit_code.map_or(Value::Null, Value::from),
    );
    reply.insert("killed".to_owned(), Value::Bool(killed));
    reply.insert("output".to_owned(), Value::String(report.output.clone()));
    reply
}

fn lock(handles: &Mutex<Vec<yi_tools::JobId>>) -> std::sync::MutexGuard<'_, Vec<yi_tools::JobId>> {
    handles
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl HostHandlers for HostRegistry {
    fn dispatch(&self, request_type: &str, payload: Map<String, Value>) -> Option<HostFuture> {
        let handler = self.handlers.get(request_type)?;
        Some(handler(payload))
    }

    /// Incident: only `exec.release` retired a handle job, so a disposed or crashed kernel
    /// left its children and their registry entries for the host's lifetime.
    fn retire(&self) {
        let registry = yi_tools::jobs::registry();
        for id in lock(&self.handles).drain(..) {
            let _a_settled_job_is_already_retired = registry.kill(id);
            let _releasing_an_unknown_job_is_the_same_absence = registry.release(id);
        }
    }
}

pub type RestoreNoticeFn = dyn Fn(&yi_types::kernel::KernelRestoreResult) + Send + Sync;

pub struct KernelServiceOptions {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub session_dir: Option<PathBuf>,
    /// the family's shared directory for objects and the blackboard (D164).
    pub family_dir: Option<PathBuf>,
    pub host: Arc<dyn HostHandlers>,
    pub on_restore: Option<Arc<RestoreNoticeFn>>,
    pub sandbox: Option<yi_tools::Sandbox>,
    pub snapshot_key: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    /// A cell's wall clock, past which it is interrupted as a cancel would; `None` is
    /// bash's ceiling, [`yi_tools::MAX_TIMEOUT_SECS`].
    pub cell_ceiling: Option<std::time::Duration>,
}

/// A session's snapshot files, beside the root's, prefixed with its id: the sessions
/// directory is read flat by its consumers, so no subdirectory appears in it.
pub fn snapshot_paths(base: &std::path::Path, key: Option<&str>) -> (PathBuf, PathBuf) {
    let (snapshot, manifest) = (
        yi_kernel::snapshot::snapshot_path_in(base),
        yi_kernel::snapshot::manifest_path_in(base),
    );
    let prefixed = |path: PathBuf, id: &str| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        name.map_or(path.clone(), |name| base.join(format!("{id}.{name}")))
    };
    match key {
        Some(id) if !id.is_empty() => (prefixed(snapshot, id), prefixed(manifest, id)),
        _ => (snapshot, manifest),
    }
}

/// Boots on first cell, memoizes the manager, retries a failed start, owns busy recovery.
pub struct KernelService {
    options: KernelServiceOptions,
    sandbox: tokio::sync::Mutex<Option<yi_tools::Sandbox>>,
    manager: tokio::sync::Mutex<Option<Arc<KernelManager>>>,
    last_error: Mutex<Option<String>>,
    on_death: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    died: std::sync::atomic::AtomicBool,
}

impl KernelService {
    pub fn new(options: KernelServiceOptions) -> Self {
        Self {
            sandbox: tokio::sync::Mutex::new(options.sandbox.clone()),
            options,
            manager: tokio::sync::Mutex::new(None),
            last_error: Mutex::new(None),
            on_death: Mutex::new(None),
            died: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn on_death(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.on_death.lock() {
            *slot = Some(hook);
        }
    }

    pub fn take_death(&self) -> bool {
        self.died.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    async fn died_under(&self, manager: &Arc<KernelManager>, cancelled: &CancelFlag) {
        let ours = self
            .manager
            .lock()
            .await
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, manager));
        let hook = self.on_death.lock().ok().and_then(|slot| slot.clone());
        if let (true, false, false, Some(hook)) = (ours, cancelled(), manager.is_running(), hook) {
            self.died.store(true, std::sync::atomic::Ordering::SeqCst);
            hook();
        }
    }

    /// The environment line's fact: `ready`, `booting`, `idle`, or the last boot error.
    pub fn state(&self) -> String {
        let Ok(slot) = self.manager.try_lock() else {
            return "booting".to_owned();
        };
        if slot.as_ref().is_some_and(|manager| manager.is_running()) {
            return "ready".to_owned();
        }
        match self.last_error.lock().ok().and_then(|error| error.clone()) {
            Some(error) => format!("unavailable: {}", error.lines().next().unwrap_or_default()),
            None => "idle (boots on the first ipython call)".to_owned(),
        }
    }

    pub async fn set_sandbox(&self, sandbox: Option<yi_tools::Sandbox>) {
        let mut slot = self.sandbox.lock().await;
        if *slot == sandbox {
            return;
        }
        *slot = sandbox;
        drop(slot);
        self.kill().await;
    }

    fn kernel_wrap(&self, sandbox: Option<&yi_tools::Sandbox>) -> Option<(String, Vec<String>)> {
        let sandbox = sandbox.filter(|_| yi_tools::Sandbox::available())?;
        let mut profile = sandbox.clone();
        // Only what the kernel side writes under ~/.yi; the venv stays read-only.
        let yi = self.options.home.join(".yi");
        profile.writable.push(yi.join("harness"));
        profile.writable.push(yi.join("mcp"));
        profile.writable.sort();
        profile.writable.dedup();
        Some(profile.kernel_prefix())
    }

    fn kernel_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Ok(bin) = std::env::current_exe() {
            env.push(("YI_BIN".to_owned(), bin.to_string_lossy().into_owned()));
        }
        if let Some(session_dir) = &self.options.session_dir {
            env.push((
                "RLM_SESSION_DIR".to_owned(),
                session_dir.to_string_lossy().into_owned(),
            ));
        }
        if let Some(family_dir) = &self.options.family_dir {
            env.push((
                "RLM_FAMILY_DIR".to_owned(),
                family_dir.to_string_lossy().into_owned(),
            ));
        }
        env.push((
            "RLM_GLOBAL_HARNESS_STATE_DIR".to_owned(),
            self.options
                .home
                .join(".yi")
                .join("harness")
                .to_string_lossy()
                .into_owned(),
        ));
        // Set but never read by Python; the host-side depth check is
        // authoritative (design K11).
        env.push(("RLM_DEPTH".to_owned(), "0".to_owned()));
        env.push(("RLM_MAX_DEPTH".to_owned(), "1".to_owned()));
        env
    }

    /// Boot the kernel now so the first cell pays execution only. A failed
    /// prewarm stays quiet: the next cell repeats [`Self::ensure`] and reports it.
    pub async fn prewarm(&self) {
        let _first_cell_will_report = self.ensure().await;
    }

    async fn ensure(&self) -> Result<Arc<KernelManager>, String> {
        let outcome = self.ensure_inner().await;
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = outcome.as_ref().err().cloned();
        }
        outcome
    }

    async fn ensure_inner(&self) -> Result<Arc<KernelManager>, String> {
        let wrap = self.kernel_wrap(self.sandbox.lock().await.as_ref());
        // Only an on-disk session is revivable (K10). Incident: `/new`, `switch_session` and
        // `fork` swap the store under a live kernel, which kept writing under the old id.
        let key = self.options.snapshot_key.as_ref().and_then(|key| key());
        let snapshot = self.options.session_dir.as_deref().map(|dir| {
            let (path, manifest_path) = snapshot_paths(dir, key.as_deref());
            yi_kernel::client::KernelSnapshotConfig {
                path,
                manifest_path,
                max_bytes: None,
                max_variable_bytes: None,
                debounce_ms: None,
            }
        });
        let mut slot = self.manager.lock().await;
        if let Some(manager) = slot.as_ref()
            && manager.is_running()
            && manager.wrap() == wrap.as_ref()
            && manager.snapshot_path() == snapshot.as_ref().map(|config| config.path.as_path())
        {
            return Ok(Arc::clone(manager));
        }
        if let Some(old) = slot.take() {
            old.dispose().await;
        }
        let snapshot_existed = snapshot
            .as_ref()
            .is_some_and(|config| config.path.is_file());
        let manager = Arc::new(KernelManager::new(KernelOptions {
            python: None,
            cwd: Some(self.options.cwd.clone()),
            env: self.kernel_env(),
            username: "yi".to_owned(),
            home: self.options.home.clone(),
            runtime_source_dir: yi_kernel::bootstrap::default_runtime_source_dir(),
            host: Some(Arc::clone(&self.options.host)),
            on_progress: Some(Arc::new(|message: &str| eprintln!("{message}"))),
            snapshot,
            wrap,
        })?);
        manager.start().await?;
        // Revive before the bootstrap cell, so the bootstrap overwrites live
        // handles (rlm, skills) on top of anything restored.
        let pending_restore = if snapshot_existed {
            Some(manager.restore_state().await.unwrap_or_default())
        } else {
            None
        };
        let imports: Vec<&str> = yi_kernel::bootstrap::PYTHON_SKILLS
            .iter()
            .map(|(import_name, _)| *import_name)
            .collect();
        let bootstrap = manager
            .execute(&rlm_bootstrap_code(&imports), ExecuteOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        if bootstrap.status != yi_types::kernel::ExecuteStatus::Ok {
            let mut details = bootstrap.stderr;
            if let Some(error) = bootstrap.error {
                if !details.is_empty() {
                    details.push('\n');
                }
                details.push_str(&error.traceback.join("\n"));
            }
            manager.dispose().await;
            return Err(format!(
                "Failed to initialize rlm runtime in the IPython kernel:\n{details}"
            ));
        }
        // Only tell the model what was revived once the kernel is actually
        // usable — a restore notice must never outlive a failed bootstrap.
        if let (Some(restore), Some(on_restore)) = (pending_restore, &self.options.on_restore) {
            on_restore(&restore);
        }
        *slot = Some(Arc::clone(&manager));
        Ok(manager)
    }

    pub async fn kill(&self) {
        let manager = self.manager.lock().await.take();
        if let Some(manager) = manager {
            manager.dispose().await;
        }
        self.options.host.retire();
    }

    /// A peek, never a boot: post-compaction sync must not spawn a kernel
    /// just to report on one.
    async fn manager_if_running(&self) -> Option<Arc<KernelManager>> {
        let slot = self.manager.lock().await;
        slot.as_ref()
            .filter(|manager| manager.is_running())
            .map(Arc::clone)
    }

    /// Prune oversized variables, list what survives, return the model-facing
    /// notice — None when no kernel is live to report on.
    pub async fn sync_after_compaction(&self) -> Option<String> {
        let manager = self.manager_if_running().await?;
        let pruned = manager
            .prune_oversized_variables()
            .await
            .map(|result| result.pruned)
            .unwrap_or_default();
        let names = manager.list_namespace_names().await;
        if names.is_none() && !manager.is_running() {
            return None;
        }
        let detail = match &names {
            None => String::new(),
            Some(names) if names.is_empty() => " You have not defined any names yet.".to_owned(),
            Some(names) => format!(" These names are still defined: {}.", names.join(", ")),
        };
        let pruned_detail = if pruned.is_empty() {
            String::new()
        } else {
            format!(
                " Variables above the per-variable snapshot limit were removed: {}.",
                pruned.join(", ")
            )
        };
        Some(format!(
            "<ipython_state>\nYour IPython kernel persisted through compaction; its remaining variables, imports, and helpers are still available.{pruned_detail}{detail}\n</ipython_state>"
        ))
    }

    pub async fn dispose(&self) {
        self.kill().await;
    }

    pub async fn execute_user_cell(&self, code: &str, cancelled: &CancelFlag) -> ToolOutput {
        match self.execute_async(code, cancelled).await {
            Ok(outcome) => yi_tools::cell_output(code, outcome),
            Err(message) => yi_tools::error_output(message),
        }
    }

    async fn execute_async(
        &self,
        code: &str,
        cancelled: &CancelFlag,
    ) -> Result<KernelCellOutcome, String> {
        let mut kernel_restarted = false;
        let ceiling = self
            .options
            .cell_ceiling
            .unwrap_or(std::time::Duration::from_secs(yi_tools::MAX_TIMEOUT_SECS));
        loop {
            let manager = self.ensure().await?;
            let abort = AbortFlag::default();
            let watcher = {
                let abort = abort.clone();
                let cancelled = Arc::clone(cancelled);
                // Started after the boot, so a cold venv build is not charged to the cell;
                // a ceiling past `Instant`'s range is no clock at all.
                let ceiling_at = std::time::Instant::now().checked_add(ceiling);
                tokio::spawn(async move {
                    while !cancelled() && ceiling_at.is_none_or(|at| std::time::Instant::now() < at)
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    abort.fire();
                })
            };
            let outcome = manager
                .execute(
                    code,
                    ExecuteOptions {
                        abort: Some(abort),
                        ..ExecuteOptions::default()
                    },
                )
                .await;
            watcher.abort();
            match outcome {
                Ok(result) => {
                    return Ok(KernelCellOutcome {
                        result,
                        kernel_restarted,
                    });
                }
                // Headless busy recovery: kill + fresh kernel + restart notice
                // into model context; no UI to ask yet.
                Err(ExecuteError::BusyAfterInterrupt) => {
                    if cancelled() {
                        return Err(ExecuteError::BusyAfterInterrupt.to_string());
                    }
                    self.kill().await;
                    kernel_restarted = true;
                }
                Err(error) => {
                    self.died_under(&manager, cancelled).await;
                    return Err(error.to_string());
                }
            }
        }
    }
}

pub fn ipython_tool(service: Arc<KernelService>) -> Arc<dyn yi_tools::Tool> {
    Arc::new(yi_tools::IpythonTool { bridge: service })
}

impl KernelBridge for KernelService {
    fn execute_cell(
        &self,
        code: &str,
        cancelled: &CancelFlag,
    ) -> Result<KernelCellOutcome, String> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "ipython requires a tokio runtime".to_owned())?;
        handle.block_on(self.execute_async(code, cancelled))
    }
}

pub(crate) const VARIABLE_MARKER: &str = "__yi_kernel_var__";
pub(crate) const VARIABLE_MAX_CHARS: usize = 8_192;
const VARIABLE_NAME_MAX_BYTES: usize = 128;

/// Invariant: an ASCII Python identifier, never a dotted path: the name is
/// interpolated into a cell, and attribute access runs arbitrary code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableName(String);

impl VariableName {
    pub fn parse(raw: &str) -> Result<Self, VariableReadError> {
        let head_ok = raw
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
        let body_ok = raw
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '_');
        if head_ok && body_ok && raw.len() <= VARIABLE_NAME_MAX_BYTES {
            return Ok(Self(raw.to_owned()));
        }
        Err(VariableReadError::NotAnIdentifier {
            name: raw.to_owned(),
            max: VARIABLE_NAME_MAX_BYTES,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VariableName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VariableReadError {
    #[error("{name:?} is not an ASCII Python identifier of 1 to {max} bytes")]
    NotAnIdentifier { name: String, max: usize },
    #[error("no IPython kernel is running for this agent")]
    NotRunning,
    #[error("the kernel could not be read: {detail}")]
    Cell { detail: String },
    #[error("repr({name}) raised {python}")]
    Unreadable { name: VariableName, python: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VariableReply {
    Missing,
    Value { text: String, chars: usize },
    Unreadable { python: String },
}

fn render_value(text: String, chars: usize) -> String {
    let shown = text.chars().count();
    if chars <= shown {
        return text;
    }
    format!("{text}\n[... truncated: {shown} of {chars} chars ...]")
}

impl KernelService {
    /// Invariant: reads a kernel already running — a fetch must not boot one.
    pub async fn read_variable(
        &self,
        name: &VariableName,
        page: Option<crate::fetch::Page>,
    ) -> Result<Option<(String, Option<usize>)>, VariableReadError> {
        let reply = self.variable_cell(name, read_variable_code(name, page));
        Ok(reply.await?.map(|(text, chars)| match page {
            None => (render_value(text, chars), None),
            Some(page) => {
                let end = page.offset.saturating_add(text.chars().count());
                (text, (end < chars).then_some(end))
            }
        }))
    }

    /// The variable dilled to `path` (D164): `Some(bytes)` when it exists.
    pub async fn dump_variable(
        &self,
        name: &VariableName,
        path: &std::path::Path,
    ) -> Result<Option<u64>, VariableReadError> {
        self.variable_cell(name, dump_variable_code(name, path))
            .await
            .map(|reply| reply.map(|(_, chars)| u64::try_from(chars).unwrap_or(u64::MAX)))
    }

    async fn variable_cell(
        &self,
        name: &VariableName,
        code: String,
    ) -> Result<Option<(String, usize)>, VariableReadError> {
        let manager = self
            .manager_if_running()
            .await
            .ok_or(VariableReadError::NotRunning)?;
        let abort = AbortFlag::default();
        let timer = {
            let abort = abort.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(
                    yi_kernel::KERNEL_STATE_LISTING_TIMEOUT_MS,
                ))
                .await;
                abort.fire();
            })
        };
        let outcome = manager
            .execute(
                &code,
                ExecuteOptions {
                    abort: Some(abort),
                    internal: true,
                    ..ExecuteOptions::default()
                },
            )
            .await;
        timer.abort();
        let result = outcome.map_err(|error| VariableReadError::Cell {
            detail: error.to_string(),
        })?;
        if result.status != yi_types::kernel::ExecuteStatus::Ok {
            return Err(VariableReadError::Cell {
                detail: result
                    .error
                    .map(|error| error.evalue)
                    .unwrap_or(result.stderr),
            });
        }
        match parse_variable_reply(&result.stdout) {
            None => Err(VariableReadError::Cell {
                detail: format!("the read cell printed no {VARIABLE_MARKER} line"),
            }),
            Some(VariableReply::Missing) => Ok(None),
            Some(VariableReply::Unreadable { python }) => Err(VariableReadError::Unreadable {
                name: name.clone(),
                python,
            }),
            Some(VariableReply::Value { text, chars }) => Ok(Some((text, chars))),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_snapshot_is_keyed_by_its_session() {
        let base = std::path::Path::new("/tmp/sessions");
        let (keyed, manifest) = super::snapshot_paths(base, Some("01a0"));
        let (bare, _) = super::snapshot_paths(base, None);
        assert_eq!(
            keyed.parent(),
            Some(base),
            "flat beside the sessions, never a subdirectory"
        );
        assert!(
            keyed
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("01a0."))
        );
        assert!(manifest.to_string_lossy().ends_with(".json"));
        assert_eq!(bare, yi_kernel::snapshot::snapshot_path_in(base));
        assert_eq!(super::snapshot_paths(base, Some("")).0, bare);
    }

    use super::*;
    use std::error::Error;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn only_a_bare_ascii_identifier_is_a_variable_name() -> TestResult {
        for good in ["x", "_x", "a1", "Data_2", "_", "__doc__"] {
            let parsed = VariableName::parse(good)?;
            assert_eq!(parsed.as_str(), good);
        }
        let long = "a".repeat(VARIABLE_NAME_MAX_BYTES.saturating_add(1));
        for bad in [
            "",
            "a.b",
            "os.system",
            "a b",
            "1a",
            "x;import os",
            "x)",
            "x[0]",
            "__import__('os')",
            "é",
            long.as_str(),
        ] {
            assert!(
                VariableName::parse(bad).is_err(),
                "{bad:?} must be refused before it reaches a cell"
            );
        }
        Ok(())
    }

    #[test]
    fn the_read_cell_carries_the_name_as_data_not_as_code() -> TestResult {
        let code = read_variable_code(&VariableName::parse("answer")?, None);
        assert!(code.contains("name = \"answer\""), "{code}");
        assert!(code.contains("if name not in ns:"), "{code}");
        assert!(
            !code.contains("repr(answer)"),
            "the name must never be evaluated: {code}"
        );
        Ok(())
    }

    #[test]
    fn a_reply_is_parsed_into_absence_value_or_failure() -> TestResult {
        let reply = |json: &str| parse_variable_reply(&format!("noise\n{VARIABLE_MARKER}{json}\n"));
        assert_eq!(reply(r#"{"found": false}"#), Some(VariableReply::Missing));
        assert_eq!(
            reply(r#"{"found": true, "chars": 2, "text": "42"}"#),
            Some(VariableReply::Value {
                text: "42".to_owned(),
                chars: 2,
            })
        );
        assert_eq!(
            reply(r#"{"found": true, "error": "ValueError()"}"#),
            Some(VariableReply::Unreadable {
                python: "ValueError()".to_owned(),
            })
        );
        assert!(reply("not-json").is_none());
        assert!(parse_variable_reply("no marker at all").is_none());
        Ok(())
    }

    #[test]
    fn a_clipped_value_reports_what_was_hidden() {
        assert_eq!(render_value("42".to_owned(), 2), "42");
        assert_eq!(
            render_value("ab".to_owned(), 900),
            "ab\n[... truncated: 2 of 900 chars ...]"
        );
    }

    async fn exec(
        registry: &HostRegistry,
        request: &str,
        payload: Value,
    ) -> Result<Map<String, Value>, String> {
        let payload = payload.as_object().cloned().unwrap_or_default();
        match registry.dispatch(request, payload) {
            Some(future) => future.await,
            None => Err(format!("{request} is not registered")),
        }
    }

    fn exec_registry() -> HostRegistry {
        let mut registry = HostRegistry::default();
        registry.register_exec(std::env::temp_dir());
        registry
    }

    async fn settled(registry: &HostRegistry, job: &Value) -> Result<Map<String, Value>, String> {
        for _attempt in 0u16..500 {
            let report = exec(registry, "exec.poll", serde_json::json!({"job_id": job})).await?;
            if report.get("running") == Some(&Value::Bool(false)) {
                return Ok(report);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Err("job under test never settled".to_owned())
    }

    #[tokio::test]
    async fn an_exec_job_spawns_tails_and_releases() -> TestResult {
        let registry = exec_registry();
        let spawned = exec(
            &registry,
            "exec.spawn",
            serde_json::json!({"command": "printf hello"}),
        )
        .await?;
        let job = spawned.get("job_id").ok_or("no job_id")?.clone();
        let report = settled(&registry, &job).await?;
        assert_eq!(report.get("exit_code"), Some(&Value::from(0)));
        assert_eq!(report.get("killed"), Some(&Value::Bool(false)));
        let chunk = exec(
            &registry,
            "exec.tail",
            serde_json::json!({"job_id": job, "cursor": 0}),
        )
        .await?;
        assert_eq!(chunk.get("text"), Some(&Value::String("hello".to_owned())));
        assert_eq!(chunk.get("dropped"), Some(&Value::from(0)));
        let released = exec(
            &registry,
            "exec.release",
            serde_json::json!({"job_id": job}),
        )
        .await?;
        assert!(
            released
                .get("output")
                .and_then(Value::as_str)
                .is_some_and(|output| output.contains("hello"))
        );
        let gone = exec(&registry, "exec.poll", serde_json::json!({"job_id": job})).await;
        assert!(gone.is_err(), "a released job must be gone");
        Ok(())
    }

    #[tokio::test]
    async fn an_exec_kill_settles_the_job_as_killed() -> TestResult {
        let registry = exec_registry();
        let spawned = exec(
            &registry,
            "exec.spawn",
            serde_json::json!({"command": "sleep 300"}),
        )
        .await?;
        let job = spawned.get("job_id").ok_or("no job_id")?.clone();
        let killed = exec(&registry, "exec.kill", serde_json::json!({"job_id": job})).await?;
        assert_eq!(
            killed.get("outcome"),
            Some(&Value::String("signalled".to_owned()))
        );
        let report = settled(&registry, &job).await?;
        assert_eq!(report.get("killed"), Some(&Value::Bool(true)));
        assert_eq!(report.get("exit_code"), Some(&Value::Null));
        let again = exec(&registry, "exec.kill", serde_json::json!({"job_id": job})).await?;
        assert_eq!(
            again.get("outcome"),
            Some(&Value::String("already_settled".to_owned()))
        );
        exec(
            &registry,
            "exec.release",
            serde_json::json!({"job_id": job}),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn exec_requests_refuse_bad_arguments_by_name() -> TestResult {
        let registry = exec_registry();
        let empty = exec(
            &registry,
            "exec.spawn",
            serde_json::json!({"command": "  "}),
        )
        .await;
        assert!(empty.is_err_and(|error| error.contains("command")));
        for request in ["exec.tail", "exec.poll", "exec.kill", "exec.release"] {
            let missing = exec(&registry, request, serde_json::json!({})).await;
            assert!(
                missing.is_err_and(|error| error.contains("job_id")),
                "{request}"
            );
        }
        let unknown = exec(
            &registry,
            "exec.poll",
            serde_json::json!({"job_id": 4_000_000_000u64}),
        )
        .await;
        assert!(unknown.is_err_and(|error| error.contains("no such job")));
        Ok(())
    }

    #[tokio::test]
    async fn disposing_the_kernel_retires_its_handle_jobs() -> TestResult {
        let registry = Arc::new(exec_registry());
        let spawned = exec(
            &registry,
            "exec.spawn",
            serde_json::json!({"command": "sleep 300"}),
        )
        .await?;
        let job = spawned.get("job_id").ok_or("no job_id")?.clone();
        let id = yi_tools::JobId(job.as_u64().ok_or("job_id is not a number")?);
        assert!(yi_tools::jobs::registry().report(id).is_some());
        let service = KernelService::new(KernelServiceOptions {
            cwd: std::env::temp_dir(),
            home: std::env::temp_dir(),
            session_dir: None,
            family_dir: None,
            host: registry,
            on_restore: None,
            sandbox: None,
            snapshot_key: None,
            cell_ceiling: None,
        });
        service.dispose().await;
        assert!(
            yi_tools::jobs::registry().report(id).is_none(),
            "a disposed kernel left its handle job in the registry"
        );
        Ok(())
    }

    fn service() -> KernelService {
        let mut registry = HostRegistry::default();
        registry.register_mcp_stubs();
        KernelService::new(KernelServiceOptions {
            cwd: std::env::temp_dir(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            session_dir: None,
            family_dir: None,
            host: Arc::new(registry),
            on_restore: None,
            sandbox: None,
            snapshot_key: None,
            cell_ceiling: None,
        })
    }

    #[tokio::test]
    async fn a_read_without_a_kernel_is_not_an_absence() -> TestResult {
        let error = service()
            .read_variable(&VariableName::parse("x")?, None)
            .await;
        assert!(matches!(error, Err(VariableReadError::NotRunning)));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "tier-2 journey: `just journeys`"]
    async fn a_live_kernel_answers_one_name_and_admits_the_rest() -> TestResult {
        let service = service();
        let manager = service.ensure().await?;
        manager
            .execute("answer = 6 * 7", ExecuteOptions::default())
            .await?;
        assert_eq!(
            service
                .read_variable(&VariableName::parse("answer")?, None)
                .await?,
            Some(("42".to_owned(), None))
        );
        assert_eq!(
            service
                .read_variable(&VariableName::parse("never_bound")?, None)
                .await?,
            None
        );
        service.dispose().await;
        Ok(())
    }
}
