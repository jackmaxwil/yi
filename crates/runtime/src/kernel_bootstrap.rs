use std::sync::Arc;

/// Binds `rlm` and `mcp` in the namespace, or a loud placeholder when the
/// runtime package is missing. [`rlm_bootstrap_code`] appends bundled skills.
pub const RLM_BOOTSTRAP_CODE: &str = r#"
import asyncio
import os as _yi_os

_yi_os.environ["NO_COLOR"] = "1"
_yi_os.environ["PIP_NO_COLOR"] = "1"
_yi_os.environ["PIP_DISABLE_PIP_VERSION_CHECK"] = "1"
get_ipython().colors = "nocolor"

try:
    import nest_asyncio as _yi_nest_asyncio
    _yi_nest_asyncio.apply()
except Exception:
    pass

# Incident: an interrupt queued its cancel on the loop without waking it, so a cell awaiting
# a 30 s sleep outlived the busy window and the kernel was killed with its namespace.
try:
    import signal as _yi_signal
    asyncio.get_event_loop().add_signal_handler(_yi_signal.SIGINT, lambda: None)
except Exception:
    pass

try:
    import rlm
    import rlm.mcp as mcp
    fetch, bash = rlm.fetch, rlm.bash
except Exception as _yi_rlm_error:
    _RLM_IMPORT_ERROR = str(_yi_rlm_error)

    class _YiMissingRlm:
        def __getattr__(self, name):
            if name.startswith("_"):
                raise AttributeError(name)
            raise RuntimeError(
                "yi-runtime is not installed in this IPython kernel. "
                "Remove ~/.yi/kernel-venv-* so yi can rebuild it, or set "
                "YI_KERNEL_PYTHON to a kernel environment with yi-runtime installed. "
                f"Import error: {_RLM_IMPORT_ERROR}"
            )

        def __call__(self, *args, **kwargs):
            return self.run(*args, **kwargs)

    rlm = _YiMissingRlm()

# Imported here so its pre_run_cell hook sees every later cell's source (plan section 8.3).
try:
    import yi
except Exception:
    pass
"#;

const SKILL_WRAPPER_CODE: &str = r#"
import importlib as _yi_importlib
import inspect as _yi_inspect
import sys as _yi_sys
import types as _yi_types

class _YiCallableSkillModule(_yi_types.ModuleType):
    async def __call__(self, *args, **kwargs):
        result = self.run(*args, **kwargs)
        if _yi_inspect.isawaitable(result):
            return await result
        return result

class _YiUnavailableSkill:
    def __init__(self, name, error):
        self.__name__ = name
        self._yi_import_error = error
        self.__doc__ = f"Python skill {name} is unavailable: {error}"

    async def run(self, *args, **kwargs):
        raise RuntimeError(
            f"Python skill {self.__name__} is unavailable in this IPython kernel. "
            f"Import error: {self._yi_import_error}"
        )

    async def __call__(self, *args, **kwargs):
        return await self.run(*args, **kwargs)

    def __repr__(self):
        return f"<unavailable Python skill {self.__name__!r}: {self._yi_import_error}>"

def _yi_wrap_skill_module(module):
    run = getattr(module, "run", None)
    if not callable(run):
        return module
    if isinstance(module, _YiCallableSkillModule):
        return module
    wrapped = _YiCallableSkillModule(module.__name__)
    wrapped.__dict__.update(module.__dict__)
    try:
        wrapped.__signature__ = _yi_inspect.signature(run)
    except Exception:
        pass
    doc = getattr(run, "__doc__", None)
    if doc:
        wrapped.__doc__ = doc
    _yi_sys.modules[module.__name__] = wrapped
    return wrapped

_SKILL_IMPORT_ERRORS = {}

for _yi_skill_name in %IMPORTS%:
    try:
        globals()[_yi_skill_name] = _yi_wrap_skill_module(
            _yi_importlib.import_module(_yi_skill_name)
        )
    except Exception as _yi_skill_error:
        _SKILL_IMPORT_ERRORS[_yi_skill_name] = str(_yi_skill_error)
        globals()[_yi_skill_name] = _YiUnavailableSkill(
            _yi_skill_name,
            str(_yi_skill_error),
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

pub const SURFACE_CODE: &str = r#"
def _yi_surface():
    import inspect
    rows = []
    for name in getattr(rlm, "__all__", []):
        obj = getattr(rlm, name, None)
        if inspect.isclass(obj) or not callable(obj):
            continue
        try:
            sig = inspect.signature(obj)
        except (TypeError, ValueError):
            continue
        empty = inspect.Parameter.empty
        params = [p.replace(annotation=empty) for p in sig.parameters.values()]
        sig = sig.replace(parameters=params, return_annotation=empty)
        rows.append(("await " if inspect.iscoroutinefunction(obj) else "") + f"rlm.{name}{sig}")
    print("\n".join(rows))
_yi_surface()
del _yi_surface
"#;

const OFFERED_BY_RLM: &[&str] = &[
    "run", "send", "request", "wait", "result", "bash", "fetch", "put", "get",
];
const OFFERED_BY_YI: &[&str] = &["Plan", "Todo", "Run", "fork_join"];

pub fn surface_line() -> String {
    format!(
        "rlm: {} · yi: {} · help(rlm) and help(yi) are exact",
        OFFERED_BY_RLM.join(", "),
        OFFERED_BY_YI.join(", ")
    )
}

pub fn surface_note(table: &str) -> String {
    format!("[rlm, shown once per session; help(rlm) and help(yi) are exact]\n{table}")
}

pub fn restart_note(lost: Option<&[String]>) -> String {
    match lost {
        None => "[IPython kernel was restarted; in-memory state was lost]".to_owned(),
        Some([]) => {
            "[IPython kernel was restarted; every name it held is defined again]".to_owned()
        }
        Some(names) => format!(
            "[IPython kernel was restarted; {} names lost: {}. Define them again before use]",
            names.len(),
            names.join(", ")
        ),
    }
}

pub type BootFn = dyn Fn(Option<&str>) + Send + Sync;

pub(crate) struct Booting(Option<Arc<BootFn>>);

impl Booting {
    pub(crate) fn new(report: Option<Arc<BootFn>>) -> Self {
        if let Some(report) = &report {
            report(Some("starting the kernel"));
        }
        Self(report)
    }

    pub(crate) fn progress(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let report = self.0.clone();
        Arc::new(move |message: &str| match &report {
            Some(report) => report(Some(message)),
            None => eprintln!("{message}"),
        })
    }
}

impl Drop for Booting {
    fn drop(&mut self) {
        if let Some(report) = &self.0 {
            report(None);
        }
    }
}
