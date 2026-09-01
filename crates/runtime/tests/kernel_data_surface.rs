use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
use yi_tools::{CancelFlag, KernelBridge};

type TestResult = Result<(), Box<dyn Error>>;

/// The kernel-side contract of `fetch(url)` and `bash(cmd)` driven by a real
/// python3 against a stubbed `rlm.host_request` — no kernel, no host.
const PY_CONTRACT: &str = r#"
import asyncio, os
import rlm as rlm_module

os.environ["RLM_SESSION_DIR"] = "/tmp/session-root"
assert rlm_module._kernel_local_name("kernel://x") == "x"
assert rlm_module._kernel_local_name("kernel://main/x") == "x"
assert rlm_module._kernel_local_name("kernel://other/x") is None
assert rlm_module._kernel_local_name("kernel://main/a/b") is None
assert rlm_module._kernel_local_name("kernel://main/") is None
assert rlm_module._kernel_local_name("local://x") is None
os.environ["RLM_SESSION_DIR"] = "/tmp/session-root/sub-abc"
assert rlm_module._kernel_local_name("kernel://x") is None
assert rlm_module._kernel_local_name("kernel://main/x") is None
del os.environ["RLM_SESSION_DIR"]
assert rlm_module._kernel_local_name("kernel://x") == "x"

try:
    rlm_module.bash("   ")
except ValueError:
    pass
else:
    raise AssertionError("bash must refuse an empty command")

calls = []
state = {"running": True, "released": False}
async def stub(request_type, payload=None):
    calls.append(request_type)
    if request_type == "exec.spawn":
        return {"job_id": 7}
    if request_type == "exec.tail":
        assert payload["cursor"] == 0
        return {"text": "hi", "next": 2, "dropped": 3}
    if request_type == "exec.poll":
        return {"running": state["running"], "exit_code": None, "killed": False}
    if request_type == "exec.kill":
        state["running"] = False
        return {"outcome": "signalled"}
    if request_type == "exec.release":
        state["released"] = True
        return {"command": "sleep 5", "output": "hi", "exit_code": None, "killed": True, "running": False}
    raise AssertionError(request_type)
rlm_module.host_request = stub

async def main():
    h = rlm_module.bash("sleep 5")
    tail = await h.tail()
    assert tail.startswith("[... 3 bytes trimmed") and tail.endswith("hi"), tail
    assert (await h.poll())["running"] is True
    report = await h.kill()
    assert report["killed"] is True
    assert state["released"], "kill must release the host-side job"
    n = len(calls)
    assert (await h) == report, "a second await returns the cached report"
    assert len(calls) == n, "a released handle makes no further host requests"
    assert (await h.tail()) == ""

    async def fetch_stub(request_type, payload=None):
        assert request_type == "fetch" and payload == {"url": "plan://p"}
        return {"text": "doc", "hash": "h", "servedBy": "plan-file"}
    rlm_module.host_request = fetch_stub
    assert (await rlm_module.fetch("plan://p")) == "doc"
    try:
        await rlm_module.fetch("")
    except TypeError:
        pass
    else:
        raise AssertionError("fetch must refuse an empty url")

asyncio.run(main())
print("contract ok")
"#;

#[expect(
    clippy::disallowed_methods,
    reason = "the contract under test is a real python process importing the shipped package"
)]
#[test]
fn the_fetch_and_bash_contract_holds_against_a_stubbed_host() -> TestResult {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let src = repo.join("python").join("yi_runtime").join("src");
    let output = std::process::Command::new("python3")
        .args(["-c", PY_CONTRACT])
        .env("PYTHONPATH", &src)
        .current_dir(std::env::temp_dir())
        .output()?;
    assert!(
        output.status.success(),
        "the kernel-side data surface broke its contract; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn service() -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    registry.register_exec(std::env::temp_dir());
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        host: Arc::new(registry),
        on_restore: None,
    }))
}

async fn cell(
    service: &Arc<KernelService>,
    code: &'static str,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_live_kernel_fetches_its_own_variables_and_runs_bash_handles() -> TestResult {
    let service = service();
    let local = cell(
        &service,
        "x = 6 * 7\nv = await fetch('kernel://x')\nw = await fetch('kernel://main/x')\nprint(type(v).__name__, v, w + 1)",
    )
    .await?;
    assert!(
        local.result.stdout.contains("int 42 43"),
        "kernel:// must return the live object: {} {}",
        local.result.stdout,
        local.result.stderr
    );
    let finished = cell(
        &service,
        "h = bash('printf hello')\nr = await h\nprint(r['exit_code'], r['output'])",
    )
    .await?;
    assert!(
        finished.result.stdout.contains("0 hello"),
        "await h must return the final report: {} {}",
        finished.result.stdout,
        finished.result.stderr
    );
    let killed = cell(
        &service,
        "h2 = bash('sleep 300')\np = await h2.poll()\nr2 = await h2.kill()\nprint(p['running'], r2['killed'])",
    )
    .await?;
    assert!(
        killed.result.stdout.contains("True True"),
        "kill must settle and release: {} {}",
        killed.result.stdout,
        killed.result.stderr
    );
    service.dispose().await;
    Ok(())
}
