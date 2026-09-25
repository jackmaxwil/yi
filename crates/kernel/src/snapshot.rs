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
    prune_oversized: bool,
) -> String {
    let out = py_path(out_path);
    let manifest = py_path(manifest_path);
    let marker = py_str(RESULT_MARKER);
    let prune = if prune_oversized { "True" } else { "False" };
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
    oversized = []
    # ponytail: keyed by id, type and getsizeof, which an in-place edit of a list, dict or
    # instance leaves unchanged, so such a variable stays skipped until rebound; a prune
    # re-measures rather than trust it.
    over_cap = _b.getattr(ip, "_yi_snapshot_over_cap", None) if ip is not None else None
    if over_cap is None:
        over_cap = {{}}
        if ip is not None:
            ip._yi_snapshot_over_cap = over_cap
    for name in [name for name in over_cap if name not in ns]:
        del over_cap[name]
    total = 0
    identify_oversized = {prune}
    for name in _b.list(ns.keys()):
        # Skip internals (dunder/underscore), IPython-injected names, and live
        # handles. A name matching a builtin (e.g. "list") is a user shadow worth
        # keeping — builtins themselves are not enumerated as user_ns keys.
        if name.startswith("_") or name in hidden or name in always_skip:
            continue
        value = ns[name]
        try:
            seen = (_b.id(value), _b.type(value), sys.getsizeof(value))
        except _b.Exception:
            seen = None
        if seen is not None and not identify_oversized and over_cap.get(name) == seen:
            skipped.append({{"name": name, "reason": "exceeds per-variable snapshot size cap"}})
            oversized.append(name)
            continue
        remaining = {max_bytes} - total
        buffer_limit = {max_variable_bytes} if identify_oversized else _b.min({max_variable_bytes}, remaining)
        buffer = SnapshotBuffer(buffer_limit)
        # Modules are pickled by reference and re-imported on restore.
        try:
            dill.dump(value, buffer)
            blob = buffer.getvalue()
            over_cap.pop(name, None)
        except SnapshotSizeLimitExceeded:
            if not identify_oversized and remaining < {max_variable_bytes}:
                skipped.append({{"name": name, "reason": "exceeds aggregate snapshot size cap"}})
            else:
                skipped.append({{"name": name, "reason": "exceeds per-variable snapshot size cap"}})
                oversized.append(name)
                if seen is not None:
                    over_cap[name] = seen
            continue
        except _b.Exception as _err:
            skipped.append({{"name": name, "reason": _b.type(_err).__name__ + ": " + _b.str(_err)[:200]}})
            continue
        if total + _b.len(blob) > {max_bytes}:
            skipped.append({{"name": name, "reason": "exceeds aggregate snapshot size cap"}})
            continue
        payload[name] = blob
        total += _b.len(blob)

    def fresh(path, mode):
        try:
            os.remove(path)
        except _b.FileNotFoundError:
            pass
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
        return os.fdopen(os.open(path, flags, 0o600), mode)

    os.makedirs(os.path.dirname({out}), exist_ok=True)
    tmp = {out} + ".tmp-" + _b.str(os.getpid())
    try:
        with fresh(tmp, "wb") as fh:
            dill.dump(payload, fh)
        os.replace(tmp, {out})
    except _b.BaseException as _err:
        try:
            os.remove(tmp)
        except _b.Exception:
            pass
        if not _b.isinstance(_err, _b.Exception):
            raise
        _b.print({marker} + json.dumps({{"error": "write failed: " + _b.str(_err)}}))
        return

    bytes_written = os.path.getsize({out})
    saved = _b.sorted(payload.keys())
    pruned = _b.sorted(name for name in oversized if name in ns) if identify_oversized else []
    manifest = {{
        "version": 1,
        "savedNames": saved,
        "skipped": skipped,
        "pruned": pruned,
        "bytes": bytes_written,
        "pythonVersion": sys.version.split()[0],
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }}
    manifest_tmp = {manifest} + ".tmp-" + _b.str(os.getpid())
    try:
        with fresh(manifest_tmp, "w") as fh:
            json.dump(manifest, fh)
        os.replace(manifest_tmp, {manifest})
    except _b.Exception:
        try:
            os.remove(manifest_tmp)
        except _b.Exception:
            pass
    pruned_ids = {{_b.id(ns[name]) for name in pruned}}
    while True:
        try:
            for name in pruned:
                if name in ns:
                    del ns[name]
            output_cache = ns.get("Out")
            if _b.isinstance(output_cache, _b.dict):
                for key in _b.list(output_cache.keys()):
                    if _b.id(output_cache[key]) in pruned_ids:
                        del output_cache[key]
            for name in hidden:
                if name in ns and _b.id(ns[name]) in pruned_ids:
                    del ns[name]
            break
        except _b.KeyboardInterrupt:
            # Deletion is idempotent. Finish the short critical section so a
            # snapshot timeout cannot leave only some purge candidates live.
            continue
    _b.print({marker} + json.dumps({{"saved": saved, "skipped": skipped, "pruned": pruned, "bytes": bytes_written}}))


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

/// Marker-line list of live user-defined names, filtered like the snapshot.
/// Never raises.
pub fn build_list_names_code() -> String {
    let marker = py_str(RESULT_MARKER);
    format!(
        r#"def _yi_list_state_names():
    import builtins as _b, json
    ip = None
    try:
        ip = get_ipython()  # noqa: F821 (injected by IPython)
    except _b.Exception:
        ip = None
    ns = ip.user_ns if ip is not None else _b.globals()
    hidden = _b.set(_b.getattr(ip, "user_ns_hidden", {{}}) or {{}}) if ip is not None else _b.set()
    always_skip = {{"rlm", "mcp", "asyncio", "In", "Out", "get_ipython", "exit", "quit", "open"}}
    names = []
    for name in _b.list(ns.keys()):
        if name.startswith("_") or name in hidden or name in always_skip:
            continue
        names.append(name)
    _b.print({marker} + json.dumps({{"names": _b.sorted(names)}}))


try:
    _yi_list_state_names()
finally:
    del _yi_list_state_names"#
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

/// Sorted list of live user-defined names, or None if the marker was
/// absent/invalid.
pub fn parse_list_names(stdout: &str) -> Option<Vec<String>> {
    let value = marker_value(stdout)?;
    let names = value.get("names")?.as_array()?;
    Some(
        names
            .iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect(),
    )
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
            let _ = manager.capture_snapshot(true, false).await;
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
        self.capture_snapshot(false, false).await
    }

    /// Persist the namespace, then remove variables above the per-variable cap
    /// from the live namespace (post-compaction RAM relief, design K10).
    pub async fn prune_oversized_variables(&self) -> Option<KernelSnapshotResult> {
        self.capture_snapshot(true, true).await
    }

    async fn capture_snapshot(
        &self,
        bounded: bool,
        prune_oversized: bool,
    ) -> Option<KernelSnapshotResult> {
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
            prune_oversized,
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

    /// Revive a snapshotted namespace. Call after start() and before the runtime bootstrap,
    /// which refreshes live handles over anything restored. Never fails a boot.
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

    /// Live user-defined top-level names, or None if the kernel isn't
    /// running. Never fails a caller; bounded like a snapshot cell.
    pub async fn list_namespace_names(&self) -> Option<Vec<String>> {
        if !self.is_running() {
            return None;
        }
        let code = build_list_names_code();
        match self
            .execute_internal(&code, Some(crate::KERNEL_STATE_LISTING_TIMEOUT_MS))
            .await
        {
            Ok(result) if result.status == ExecuteStatus::Ok => parse_list_names(&result.stdout),
            Ok(result) => {
                let detail = result
                    .error
                    .map(|error| error.evalue)
                    .unwrap_or(result.stderr);
                self.inner
                    .diagnostic(&format!("namespace listing failed: {detail}"));
                None
            }
            Err(error) => {
                self.inner
                    .diagnostic(&format!("namespace listing error: {error}"));
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
            false,
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
