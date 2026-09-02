use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_kernel::client::{
    AbortFlag, ExecuteError, ExecuteOptions, HostFuture, HostHandlers, KernelManager, KernelOptions,
};
use yi_tools::{CancelFlag, KernelBridge, KernelCellOutcome};

/// Binds `rlm` and `mcp` in the namespace, or a loud placeholder when the
/// runtime package is missing. [`rlm_bootstrap_code`] appends bundled skills.
pub const RLM_BOOTSTRAP_CODE: &str = r#"
import asyncio
import os as _prime_agent_os

_prime_agent_os.environ["NO_COLOR"] = "1"
get_ipython().colors = "nocolor"

try:
    import nest_asyncio as _prime_agent_nest_asyncio
    _prime_agent_nest_asyncio.apply()
except Exception:
    pass

try:
    import rlm as _prime_agent_rlm_module
    rlm = _prime_agent_rlm_module.rlm
    import rlm.mcp as mcp
except Exception as _prime_agent_rlm_error:
    _PRIME_AGENT_RLM_IMPORT_ERROR = str(_prime_agent_rlm_error)

    class _PrimeAgentMissingRlm:
        def _raise_missing(self):
            raise RuntimeError(
                "yi-runtime is not installed in this IPython kernel. "
                "Remove ~/.yi/kernel-venv so yi can rebuild it, or set "
                "YI_KERNEL_PYTHON to a kernel environment with yi-runtime installed. "
                f"Import error: {_PRIME_AGENT_RLM_IMPORT_ERROR}"
            )

        async def run(self, prompt, **kwargs):
            self._raise_missing()

        async def find_models(self, query="", limit=8):
            self._raise_missing()

        async def list_subagents(self):
            self._raise_missing()

        async def delete_subagent(self, target):
            self._raise_missing()

        async def __call__(self, prompt, **kwargs):
            return await self.run(prompt, **kwargs)

    rlm = _PrimeAgentMissingRlm()
"#;

const SKILL_WRAPPER_CODE: &str = r#"
import importlib as _prime_agent_importlib
import inspect as _prime_agent_inspect
import sys as _prime_agent_sys
import types as _prime_agent_types

class _PrimeAgentCallableSkillModule(_prime_agent_types.ModuleType):
    async def __call__(self, *args, **kwargs):
        result = self.run(*args, **kwargs)
        if _prime_agent_inspect.isawaitable(result):
            return await result
        return result

class _PrimeAgentUnavailableSkill:
    def __init__(self, name, error):
        self.__name__ = name
        self._prime_agent_import_error = error
        self.__doc__ = f"Python skill {name} is unavailable: {error}"

    async def run(self, *args, **kwargs):
        raise RuntimeError(
            f"Python skill {self.__name__} is unavailable in this IPython kernel. "
            f"Import error: {self._prime_agent_import_error}"
        )

    async def __call__(self, *args, **kwargs):
        return await self.run(*args, **kwargs)

    def __repr__(self):
        return f"<unavailable Python skill {self.__name__!r}: {self._prime_agent_import_error}>"

def _prime_agent_wrap_skill_module(module):
    run = getattr(module, "run", None)
    if not callable(run):
        return module
    if isinstance(module, _PrimeAgentCallableSkillModule):
        return module
    wrapped = _PrimeAgentCallableSkillModule(module.__name__)
    wrapped.__dict__.update(module.__dict__)
    try:
        wrapped.__signature__ = _prime_agent_inspect.signature(run)
    except Exception:
        pass
    doc = getattr(run, "__doc__", None)
    if doc:
        wrapped.__doc__ = doc
    _prime_agent_sys.modules[module.__name__] = wrapped
    return wrapped

_PRIME_AGENT_SKILL_IMPORT_ERRORS = {}

for _prime_agent_skill_name in %IMPORTS%:
    try:
        globals()[_prime_agent_skill_name] = _prime_agent_wrap_skill_module(
            _prime_agent_importlib.import_module(_prime_agent_skill_name)
        )
    except Exception as _prime_agent_skill_error:
        _PRIME_AGENT_SKILL_IMPORT_ERRORS[_prime_agent_skill_name] = str(_prime_agent_skill_error)
        globals()[_prime_agent_skill_name] = _PrimeAgentUnavailableSkill(
            _prime_agent_skill_name,
            str(_prime_agent_skill_error),
        )
"#;

/// The base cell plus a skill-module wrapper per bundled Python skill.
pub fn rlm_bootstrap_code(import_names: &[&str]) -> String {
    if import_names.is_empty() {
        return RLM_BOOTSTRAP_CODE.trim().to_owned();
    }
    let imports = serde_json::to_string(import_names).unwrap_or_else(|_| "[]".to_owned());
    format!(
        "{}\n{}",
        RLM_BOOTSTRAP_CODE.trim(),
        SKILL_WRAPPER_CODE.replace("%IMPORTS%", &imports).trim()
    )
}

pub type HostHandlerFn = dyn Fn(Map<String, Value>) -> HostFuture + Send + Sync;

/// Registered by the runtime, dispatched by yi-kernel.
#[derive(Default)]
pub struct HostRegistry {
    handlers: HashMap<String, Arc<HostHandlerFn>>,
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

    /// `mcp.config` returns `{}` (the Python side raises its own KeyError),
    /// `mcp.refresh` throws, and `mcp.begin_login` is never registered: a 401
    /// must not open a browser.
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

impl HostHandlers for HostRegistry {
    fn dispatch(&self, request_type: &str, payload: Map<String, Value>) -> Option<HostFuture> {
        let handler = self.handlers.get(request_type)?;
        Some(handler(payload))
    }
}

pub type RestoreNoticeFn = dyn Fn(&yi_types::kernel::KernelRestoreResult) + Send + Sync;

pub struct KernelServiceOptions {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub session_dir: Option<PathBuf>,
    pub host: Arc<dyn HostHandlers>,
    pub on_restore: Option<Arc<RestoreNoticeFn>>,
    pub sandbox: Option<yi_tools::Sandbox>,
}

/// Boots on first cell, memoizes the running manager, retries after a failed
/// start, and owns the busy-kernel recovery path.
pub struct KernelService {
    options: KernelServiceOptions,
    sandbox: tokio::sync::Mutex<Option<yi_tools::Sandbox>>,
    manager: tokio::sync::Mutex<Option<Arc<KernelManager>>>,
}

impl KernelService {
    pub fn new(options: KernelServiceOptions) -> Self {
        Self {
            sandbox: tokio::sync::Mutex::new(options.sandbox.clone()),
            options,
            manager: tokio::sync::Mutex::new(None),
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
        let wrap = self.kernel_wrap(self.sandbox.lock().await.as_ref());
        let mut slot = self.manager.lock().await;
        if let Some(manager) = slot.as_ref()
            && manager.is_running()
            && manager.wrap() == wrap.as_ref()
        {
            return Ok(Arc::clone(manager));
        }
        if let Some(old) = slot.take() {
            old.dispose().await;
        }
        // Only sessions with an on-disk directory get a revivable snapshot
        // (design K10) — prime's artifact-dir gate.
        let snapshot = self.options.session_dir.as_deref().map(|dir| {
            yi_kernel::client::KernelSnapshotConfig {
                path: yi_kernel::snapshot::snapshot_path_in(dir),
                manifest_path: yi_kernel::snapshot::manifest_path_in(dir),
                max_bytes: None,
                max_variable_bytes: None,
                debounce_ms: None,
            }
        });
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

    async fn execute_async(
        &self,
        code: &str,
        cancelled: &CancelFlag,
    ) -> Result<KernelCellOutcome, String> {
        let mut kernel_restarted = false;
        loop {
            let manager = self.ensure().await?;
            let abort = AbortFlag::default();
            let watcher = {
                let abort = abort.clone();
                let cancelled = Arc::clone(cancelled);
                tokio::spawn(async move {
                    while !cancelled() {
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
                // into model context (prime's "kill" choice; no UI to ask yet).
                Err(ExecuteError::BusyAfterInterrupt) => {
                    if cancelled() {
                        return Err(ExecuteError::BusyAfterInterrupt.to_string());
                    }
                    self.kill().await;
                    kernel_restarted = true;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

pub fn ipython_tool(service: Arc<KernelService>) -> Arc<dyn yi_tools::Tool> {
    Arc::new(yi_tools::IpythonTool { bridge: service })
}

pub fn restore_notice_text(restore: &yi_types::kernel::KernelRestoreResult) -> String {
    let mut lines = vec!["<ipython_state_restored>".to_owned()];
    if restore.restored.is_empty() {
        lines.push(
            "Your previous IPython kernel state could not be revived; the kernel is starting fresh, so re-create any variables, imports, or loaded data you need.".to_owned(),
        );
    } else {
        lines.push(format!(
            "Your IPython kernel state was revived from your previous session. These names are available again: {}.",
            restore.restored.join(", ")
        ));
    }
    if !restore.failed.is_empty() {
        let names: Vec<&str> = restore
            .failed
            .iter()
            .map(|failure| failure.name.as_str())
            .collect();
        lines.push(format!(
            "These could not be restored and must be recreated if needed: {}.",
            names.join(", ")
        ));
    }
    lines.push("</ipython_state_restored>".to_owned());
    lines.join("\n")
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
