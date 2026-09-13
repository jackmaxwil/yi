#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use yi_runtime::plan::ops::{Actor, Delegate, PlanEngine};
use yi_runtime::plan::store::PlanStore;
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
        assert request_type == "fetch" and payload == {"url": "plan://p", "object": False}
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

struct NoChildren;

impl Delegate for NoChildren {
    fn spawn(
        &self,
        _at: &yi_types::plan::doc::TodoAddr,
        _delegation: &yi_types::plan::doc::Delegation,
    ) -> Result<yi_types::plan::doc::AgentId, String> {
        Err("no children in this journey".to_owned())
    }

    fn reap(
        &self,
        _agent: &yi_types::plan::doc::AgentId,
        _supplied: &[yi_types::url::Url],
    ) -> Result<Option<yi_types::url::Url>, String> {
        Ok(None)
    }

    fn follow_up(&self, _dispatched: &[yi_types::plan::doc::TodoLabel], _held: usize) {}
}

fn service() -> Arc<KernelService> {
    service_with(HostRegistry::default())
}

fn service_with(mut registry: HostRegistry) -> Arc<KernelService> {
    registry.register_mcp_stubs();
    registry.register_exec(std::env::temp_dir());
    Arc::new(KernelService::new(KernelServiceOptions {
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

/// Guards the `PYTHON_SKILLS` deletion and the `plan.op` registration: put the skill
/// back and the import succeeds; unregister the request and the init has no answer.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn the_plan_skill_is_gone_and_plan_op_answers() -> TestResult {
    let dir = Scratch::new("yi-kernel-plan-op")?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(NoChildren),
    ));
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(engine, Actor::Owner, &mut registry);
    let service = service_with(registry);
    let outcome = cell(
        &service,
        concat!(
            "import rlm\n",
            "try:\n",
            "    import plan\n",
            "    print('plan importable')\n",
            "except ImportError:\n",
            "    print('plan gone')\n",
            "r = await rlm.host_request('plan.op', {'request_id': 't2-1', 'op': 'init', ",
            "'args': {'goal': 'prove the request answers', 'todos': [{'label': 'answer'}]}})\n",
            "print(r['ok'], r['revision'], r['text'].splitlines()[0])\n",
        ),
    )
    .await?;
    assert!(
        outcome.result.stdout.contains("plan gone"),
        "the kernel-side plan skill must be gone: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    assert!(
        outcome.result.stdout.contains("True 1 plan "),
        "plan.op must open a plan through rlm: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    service.dispose().await;
    Ok(())
}
