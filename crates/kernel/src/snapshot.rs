use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::kernel::{ExecuteResult, ExecuteStatus, KernelRestoreResult, KernelSnapshotResult};

use crate::client::{AbortFlag, ExecuteError, ExecuteOptions, KernelManager};
use crate::{
    DEFAULT_SNAPSHOT_DEBOUNCE_MS, SNAPSHOT_EXECUTION_TIMEOUT_MS, SNAPSHOT_MAX_OUTPUT_CHARS,
};

pub const DEFAULT_SNAPSHOT_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_SNAPSHOT_MAX_VARIABLE_BYTES: u64 = 16 * 1024 * 1024;

const KERNEL_STATE_BASENAME: &str = "kernel-state";
// Lowercase deliberately: the env-surface guardrail scans for YI_[A-Z_]+ and
// this marker is a stdout sentinel, not an env var.
const RESULT_MARKER: &str = "__yi_kernel_state__";

pub fn snapshot_path_in(artifact_dir: &Path) -> PathBuf {
    artifact_dir.join(format!("{KERNEL_STATE_BASENAME}.dill"))
}

pub fn manifest_path_in(artifact_dir: &Path) -> PathBuf {
    artifact_dir.join(format!("{KERNEL_STATE_BASENAME}.json"))
}

// JSON string escaping is a valid subset of Python string-literal escaping.
fn py_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn py_path(path: &Path) -> String {
    py_str(&path.to_string_lossy())
}

pub fn build_snapshot_code(
    out_path: &Path,
    manifest_path: &Path,
    max_bytes: u64,
    max_variable_bytes: u64,
) -> String {
    let out = py_path(out_path);
    let manifest = py_path(manifest_path);
    let marker = py_str(RESULT_MARKER);
    // All builtins are sourced via the locally-imported _b alias so the helper
    // keeps working even when the user namespace shadows names like list/open.
    format!(
        r#"def _yi_snapshot_state():
    import builtins as _b, io, json, os, sys, datetime
    try:
        import dill
    except _b.Exception as _err:
        _b.print({marker} + json.dumps({{"error": "dill unavailable: " + _b.str(_err)}}))
        return
    dill.settings["recurse"] = True

    ip = None
    try:
        ip = get_ipython()  # noqa: F821 (injected by IPython)
    except _b.Exception:
        ip = None
    ns = ip.user_ns if ip is not None else _b.globals()
    hidden = _b.set(_b.getattr(ip, "user_ns_hidden", {{}}) or {{}}) if ip is not None else _b.set()
    # rlm, mcp, and asyncio are re-created by the kernel bootstrap on every
    # start; never snapshot them.
    always_skip = {{"rlm", "mcp", "asyncio", "In", "Out", "get_ipython", "exit", "quit", "open"}}

    class SnapshotSizeLimitExceeded(_b.Exception):
        pass

    class SnapshotBuffer(io.BytesIO):
        def __init__(self, limit):
            io.BytesIO.__init__(self)
            self.limit = limit

        def write(self, chunk):
            if self.tell() + _b.len(chunk) > self.limit:
                raise SnapshotSizeLimitExceeded()
            return io.BytesIO.write(self, chunk)

    payload = {{}}
    skipped = []
    total = 0
    for name in _b.list(ns.keys()):
        # Skip internals (dunder/underscore), IPython-injected names, and live
        # handles. A name matching a builtin (e.g. "list") is a user shadow worth
        # keeping — builtins themselves are not enumerated as user_ns keys.
        if name.startswith("_") or name in hidden or name in always_skip:
            continue
        value = ns[name]
        remaining = {max_bytes} - total
        buffer = SnapshotBuffer(_b.min({max_variable_bytes}, remaining))
        # Modules are pickled by reference and re-imported on restore.
        try:
            dill.dump(value, buffer)
            blob = buffer.getvalue()
        except SnapshotSizeLimitExceeded:
            if remaining < {max_variable_bytes}:
                skipped.append({{"name": name, "reason": "exceeds aggregate snapshot size cap"}})
            else:
                skipped.append({{"name": name, "reason": "exceeds per-variable snapshot size cap"}})
            continue
        except _b.Exception as _err:
            skipped.append({{"name": name, "reason": _b.type(_err).__name__ + ": " + _b.str(_err)[:200]}})
            continue
        if total + _b.len(blob) > {max_bytes}:
            skipped.append({{"name": name, "reason": "exceeds aggregate snapshot size cap"}})
            continue
        payload[name] = blob
        total += _b.len(blob)

    os.makedirs(os.path.dirname({out}), exist_ok=True)
    tmp = {out} + ".tmp"
    try:
        with _b.open(tmp, "wb") as fh:
            dill.dump(payload, fh)
        os.replace(tmp, {out})
    except _b.Exception as _err:
        try:
            os.remove(tmp)
        except _b.Exception:
            pass
        _b.print({marker} + json.dumps({{"error": "write failed: " + _b.str(_err)}}))
        return

    bytes_written = os.path.getsize({out})
    saved = _b.sorted(payload.keys())
    manifest = {{
        "version": 1,
        "savedNames": saved,
        "skipped": skipped,
        "bytes": bytes_written,
        "pythonVersion": sys.version.split()[0],
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }}
    try:
        with _b.open({manifest}, "w") as fh:
            json.dump(manifest, fh)
    except _b.Exception:
        pass
    _b.print({marker} + json.dumps({{"saved": saved, "skipped": skipped, "bytes": bytes_written}}))


try:
    _yi_snapshot_state()
finally:
    del _yi_snapshot_state"#
    )
}

pub fn build_restore_code(in_path: &Path) -> String {
    let source = py_path(in_path);
    let marker = py_str(RESULT_MARKER);
    // Builtins via the local _b alias so a shadowed name in the user namespace
    // (list/open/print/…) can't break the restore path.
    format!(
        r#"def _yi_restore_state():
    import builtins as _b, json, os
    if not os.path.exists({source}):
        _b.print({marker} + json.dumps({{"restored": [], "failed": []}}))
        return
    try:
        import dill
    except _b.Exception as _err:
        _b.print({marker} + json.dumps({{"restored": [], "failed": [], "error": "dill unavailable: " + _b.str(_err)}}))
        return

    try:
        with _b.open({source}, "rb") as fh:
            payload = dill.load(fh)
    except _b.Exception as _err:
        _b.print({marker} + json.dumps({{"restored": [], "failed": [], "error": "load failed: " + _b.str(_err)}}))
        return
    if not _b.isinstance(payload, _b.dict):
        _b.print({marker} + json.dumps({{"restored": [], "failed": [], "error": "corrupt snapshot: not a dict"}}))
        return

    ip = None
    try:
        ip = get_ipython()  # noqa: F821
    except _b.Exception:
        ip = None
    ns = ip.user_ns if ip is not None else _b.globals()

    restored = []
    failed = []
    for name, blob in payload.items():
        try:
            ns[name] = dill.loads(blob)
            restored.append(name)
        except _b.Exception as _err:
            failed.append({{"name": name, "reason": _b.type(_err).__name__ + ": " + _b.str(_err)[:200]}})
    _b.print({marker} + json.dumps({{"restored": _b.sorted(restored), "failed": failed}}))


try:
    _yi_restore_state()
finally:
    del _yi_restore_state"#
    )
}

fn marker_line(stdout: &str) -> Option<&str> {
    let index = stdout.rfind(RESULT_MARKER)?;
    let rest = &stdout[index.saturating_add(RESULT_MARKER.len())..];
    let line = rest.lines().next()?.trim();
    (!line.is_empty()).then_some(line)
}

fn marker_value(stdout: &str) -> Option<serde_json::Value> {
    let line = marker_line(stdout)?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    (value.get("error").is_none()).then_some(value)
}

pub fn parse_snapshot_result(stdout: &str, path: &Path) -> Option<KernelSnapshotResult> {
    let mut result: KernelSnapshotResult = serde_json::from_value(marker_value(stdout)?).ok()?;
    result.path = path.to_string_lossy().into_owned();
    Some(result)
}

pub fn parse_restore_result(stdout: &str, path: &Path) -> Option<KernelRestoreResult> {
    let mut result: KernelRestoreResult = serde_json::from_value(marker_value(stdout)?).ok()?;
    result.path = path.to_string_lossy().into_owned();
    Some(result)
}

impl KernelManager {
    pub(crate) fn schedule_snapshot(&self) {
        let Some(config) = &self.inner.snapshot else {
            return;
        };
        let debounce = config.debounce_ms.unwrap_or(DEFAULT_SNAPSHOT_DEBOUNCE_MS);
        let inner = Arc::clone(&self.inner);
        let task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(debounce)).await;
            let manager = KernelManager { inner };
            let _ = manager.capture_snapshot(true).await;
        });
        if let Ok(mut timer) = self.inner.snapshot_timer.lock() {
            if let Some(previous) = timer.take() {
                previous.abort();
            }
            *timer = Some(task);
        }
    }

    /// Persist the namespace now (final flush path). Best-effort: `None` on
    /// any failure, with a diagnostic in the kernel stderr tail.
    pub async fn snapshot_state(&self) -> Option<KernelSnapshotResult> {
        self.capture_snapshot(false).await
    }

    async fn capture_snapshot(&self, bounded: bool) -> Option<KernelSnapshotResult> {
        let config = self.inner.snapshot.clone()?;
        if !self.is_running() {
            return None;
        }
        let code = build_snapshot_code(
            &config.path,
            &config.manifest_path,
            config.max_bytes.unwrap_or(DEFAULT_SNAPSHOT_MAX_BYTES),
            config
                .max_variable_bytes
                .unwrap_or(DEFAULT_SNAPSHOT_MAX_VARIABLE_BYTES),
        );
        let result = self
            .execute_internal(&code, bounded.then_some(SNAPSHOT_EXECUTION_TIMEOUT_MS))
            .await;
        match result {
            Ok(result) if result.status == ExecuteStatus::Ok => {
                parse_snapshot_result(&result.stdout, &config.path)
            }
            Ok(result) => {
                let detail = result
                    .error
                    .map(|error| error.evalue)
                    .unwrap_or(result.stderr);
                self.inner.diagnostic(&format!(
                    "state snapshot {}: {detail}",
                    if result.status == ExecuteStatus::Aborted {
                        "timed out"
                    } else {
                        "failed"
                    }
                ));
                None
            }
            Err(error) => {
                self.inner
                    .diagnostic(&format!("state snapshot error: {error}"));
                None
            }
        }
    }

    /// Revive a previously snapshotted namespace into the kernel. Call right
    /// after start() and before the runtime bootstrap, which then refreshes
    /// live handles (rlm, skills) over anything restored. Never fails a boot.
    pub async fn restore_state(&self) -> Option<KernelRestoreResult> {
        let config = self.inner.snapshot.clone()?;
        let code = build_restore_code(&config.path);
        match self.execute_internal(&code, None).await {
            Ok(result) if result.status == ExecuteStatus::Ok => {
                parse_restore_result(&result.stdout, &config.path)
            }
            Ok(result) => {
                let detail = result
                    .error
                    .map(|error| error.evalue)
                    .unwrap_or(result.stderr);
                self.inner
                    .diagnostic(&format!("state restore failed: {detail}"));
                None
            }
            Err(error) => {
                self.inner
                    .diagnostic(&format!("state restore error: {error}"));
                None
            }
        }
    }

    async fn execute_internal(
        &self,
        code: &str,
        timeout_ms: Option<u64>,
    ) -> Result<ExecuteResult, ExecuteError> {
        let abort = AbortFlag::default();
        let timer = timeout_ms.map(|timeout| {
            let abort = abort.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(timeout)).await;
                abort.fire();
            })
        });
        let outcome = self
            .execute(
                code,
                ExecuteOptions {
                    abort: Some(abort),
                    max_output_chars: Some(SNAPSHOT_MAX_OUTPUT_CHARS),
                    internal: true,
                    ..ExecuteOptions::default()
                },
            )
            .await;
        if let Some(timer) = timer {
            timer.abort();
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_line_parses_and_rejects() -> Result<(), String> {
        let path = Path::new("/tmp/kernel-state.dill");
        let ok = format!(
            "noise\n{RESULT_MARKER}{}\n",
            r#"{"saved": ["x"], "skipped": [{"name": "s", "reason": "socket"}], "bytes": 12}"#
        );
        let result =
            parse_snapshot_result(&ok, path).ok_or("valid marker line must parse".to_owned())?;
        assert_eq!(result.saved, vec!["x"]);
        assert_eq!(result.skipped[0].name, "s");
        assert_eq!(result.bytes, 12);
        assert_eq!(result.path, "/tmp/kernel-state.dill");

        assert!(
            parse_snapshot_result("no marker at all", path).is_none(),
            "absent marker must be a soft failure"
        );
        assert!(
            parse_snapshot_result(&format!("{RESULT_MARKER}not-json\n"), path).is_none(),
            "garbage after the marker must be a soft failure"
        );
        assert!(
            parse_snapshot_result(
                &format!("{RESULT_MARKER}{}", r#"{"error": "dill unavailable"}"#),
                path
            )
            .is_none(),
            "a kernel-side error line must not parse as success"
        );

        let restored = format!(
            "{RESULT_MARKER}{}",
            r#"{"restored": ["a", "b"], "failed": []}"#
        );
        let result =
            parse_restore_result(&restored, path).ok_or("restore line must parse".to_owned())?;
        assert_eq!(result.restored, vec!["a", "b"]);
        Ok(())
    }

    #[test]
    fn generated_python_carries_caps_and_paths() {
        let code = build_snapshot_code(
            Path::new("/tmp/out.dill"),
            Path::new("/tmp/out.json"),
            DEFAULT_SNAPSHOT_MAX_BYTES,
            DEFAULT_SNAPSHOT_MAX_VARIABLE_BYTES,
        );
        assert!(code.contains("268435456"), "aggregate cap must be inlined");
        assert!(
            code.contains("16777216"),
            "per-variable cap must be inlined"
        );
        assert!(code.contains("\"/tmp/out.dill\""));
        assert!(code.contains("os.replace"), "the write must be atomic");
        let restore = build_restore_code(Path::new("/tmp/out.dill"));
        assert!(restore.contains("\"/tmp/out.dill\""));
    }
}
