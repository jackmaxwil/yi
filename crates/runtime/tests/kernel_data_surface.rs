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
use yi_types::subagent::ChildExit;

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

/// The ipython description named `rlm.status()` while `rlm.__all__` left it out: every
/// `rlm.<name>` the model reads is a public name of the package its kernel imports.
#[expect(
    clippy::disallowed_methods,
    reason = "the contract under test is a real python process importing the shipped package"
)]
#[test]
fn every_rlm_name_the_model_reads_is_public_in_the_package() -> TestResult {
    let tool = yi_runtime::kernel::ipython_tool(service());
    let text = [
        tool.description(),
        yi_runtime::identity_fragment(),
        yi_runtime::doctrine_fragment(),
    ]
    .join("\n");
    let pieces: Vec<&str> = text.split("rlm.").collect();
    let mut named = std::collections::BTreeSet::new();
    for pair in pieces.windows(2) {
        let [before, after] = pair else { continue };
        if before
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            continue;
        }
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            named.insert(name);
        }
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/yi_runtime/src");
    let output = std::process::Command::new("python3")
        .args(["-c", "import json, rlm; print(json.dumps(rlm.__all__))"])
        .env("PYTHONPATH", &src)
        .current_dir(std::env::temp_dir())
        .output()?;
    let public: Vec<String> = serde_json::from_slice(&output.stdout)?;
    let missing: Vec<&String> = named.iter().filter(|name| !public.contains(name)).collect();
    assert!(
        named.len() > 5,
        "the model-facing text names rlm: {named:?}"
    );
    assert!(
        missing.is_empty(),
        "named but not in rlm.__all__: {missing:?}"
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
}

fn service() -> Arc<KernelService> {
    service_with(HostRegistry::default())
}

fn service_with(mut registry: HostRegistry) -> Arc<KernelService> {
    registry.register_mcp_stubs();
    registry.register_exec(std::env::temp_dir(), None);
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }))
}

async fn cell(
    service: &Arc<KernelService>,
    code: &str,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let (service, code) = (Arc::clone(service), code.to_owned());
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), &code, &cancelled)
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
    // D213: a huge limit is clamped to the cell's cap, in chars, and the last page names no next.
    cell(&service, "long = 'é' * 9000").await?;
    let long = yi_runtime::kernel::VariableName::parse("long")?;
    let mut seen = Vec::new();
    for offset in [0, 8192, 9002] {
        let page = yi_runtime::fetch::Page {
            offset,
            limit: usize::MAX,
        };
        let (text, next) = service
            .read_variable(&long, Some(page))
            .await?
            .ok_or("long")?;
        seen.push((text.chars().count(), next));
    }
    assert_eq!(seen, [(8192, Some(8192)), (810, None), (0, None)]);
    let (whole, next) = service.read_variable(&long, None).await?.ok_or("long")?;
    assert!(whole.ends_with("[... truncated: 8192 of 9002 chars ...]") && next.is_none());
    let dir = Scratch::new("yi-dump")?;
    let path = dir.join("long.dill");
    service.dump_variable(&long, &path).await?.ok_or("dump")?;
    let held = std::fs::File::open(&path)?;
    let before = std::fs::read(&path)?;
    cell(&service, "long = 'x'").await?;
    service.dump_variable(&long, &path).await?.ok_or("redump")?;
    assert_ne!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path)?),
        std::os::unix::fs::MetadataExt::ino(&held.metadata()?),
        "a family member reading the dump must keep the old file, not see it rewritten in place"
    );
    let mut kept = Vec::new();
    std::io::Read::read_to_end(&mut &held, &mut kept)?;
    assert_eq!(kept, before);
    service.dispose().await;
    Ok(())
}

/// Six of eight dogfood trials called `rlm.status()` or `rlm.send(...)` without `await`: a send
/// is a task from the moment it is made and goes out at the cell's next await; any other call
/// nobody took runs once after the cell, and each is named with its value.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_live_kernel_runs_each_rlm_call_once_awaited_or_not() -> TestResult {
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = HostRegistry::default();
    registry.register("rlm.status", |_payload| {
        Box::pin(async {
            let reply = serde_json::json!({"members": [{"name": "kid", "state": "running"}]});
            reply.as_object().cloned().ok_or_else(|| "reply".to_owned())
        })
    });
    registry.register("rlm.wait", |_payload| {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(serde_json::Map::new())
        })
    });
    let seen = Arc::clone(&sent);
    registry.register("agent_message.send", move |payload| {
        if let Ok(mut seen) = seen.lock() {
            seen.push(payload.get("message").cloned());
        }
        Box::pin(async { Ok(serde_json::Map::new()) })
    });
    let service = service_with(registry);
    let outcome = cell(
        &service,
        concat!(
            "import sys, time\n",
            "print('one', rlm is sys.modules['rlm'], fetch is rlm.fetch, bash is rlm.bash)\n",
            "print('bare', rlm.status())\n",
            "rlm.send('kid', 'sent bare')\n",
            "cs = [rlm.send(n, 'gathered') for n in ('a', 'b')]\nawait asyncio.gather(*cs)\n",
            "gate = asyncio.Semaphore(1)\n",
            "async def bounded(c):\n    async with gate:\n        return await c\n",
            "both = await asyncio.gather(*(bounded(rlm.status(n)) for n in ('kid', 'kid')))\n",
            "print('bounded', len(both))\n",
            "c = rlm.status()\nprint('stored', (await c)[0]['state'])\n",
            "print('chosen', (await (rlm.status('kid') if c else rlm.status('x'))).state)\n",
            "h = rlm.RLMSpawnHandle('sub-1', 'kid', '/tmp', 'faux/faux-1')\nprint('live', h.state)\n",
            "r = await rlm.bash(\n    'printf hi'\n).wait()\nprint('handle', r['exit_code'], r['output'])\n",
            "rlm.bash(\n    'printf lo'\n).wait()\n",
            "async def quick():\n    t = time.monotonic()\n    s = rlm.status()\n    await s\n",
            "    return time.monotonic() - t\n",
            "async def slow():\n    w = rlm.wait(2)\n    await w\n",
            "took, _ = await asyncio.gather(quick(), slow())\nprint('quick', took < 1)\n",
        ),
    )
    .await?;
    service.dispose().await;
    let printed = format!("{}\n{}", outcome.result.stdout, outcome.result.stderr);
    let status = "[{'name': 'kid', 'state': 'running'}]";
    for line in [
        "one True True True",
        "bare <coroutine object status at 0x",
        "bounded 2",
        "stored running",
        "chosen running",
        "live running",
        "handle 0 hi",
        "quick True",
        "rlm.send() was not awaited, so the cell got a task, not its value",
        &format!(
            "rlm.status() was not awaited, so the cell got a coroutine object, not its value; it ran once after the cell and returned {status}"
        ),
        "BashHandle.wait() was not awaited",
        "'output': 'lo'",
    ] {
        assert!(printed.contains(line), "missing {line:?}: {printed}");
    }
    assert!(!printed.contains("never awaited"), "{printed}");
    let sent = sent.lock().map_err(|error| error.to_string())?.clone();
    let [bare, a, b] =
        ["sent bare", "gathered", "gathered"].map(|text| Some(serde_json::json!(text)));
    assert_eq!(
        sent,
        [bare, a, b],
        "the bare send went out at the cell's first await: {printed}"
    );
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

/// A stub delegate that counts its spawns; `alive` is what the host can vouch for.
#[derive(Default)]
struct Kids {
    spawns: std::sync::atomic::AtomicUsize,
    alive: std::sync::atomic::AtomicBool,
    finished: std::sync::atomic::AtomicBool,
}

impl Delegate for Kids {
    fn spawn(
        &self,
        _at: &yi_types::plan::doc::TodoAddr,
        _delegation: &yi_types::plan::doc::Delegation,
    ) -> Result<yi_types::plan::doc::AgentId, String> {
        self.spawns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        yi_types::plan::doc::AgentId::new("kid").map_err(|error| error.to_string())
    }

    fn reap(
        &self,
        _agent: &yi_types::plan::doc::AgentId,
        _supplied: &[yi_types::url::Url],
    ) -> Result<Option<yi_types::url::Url>, String> {
        Ok(None)
    }
}

impl yi_runtime::plan::recovery::Liveness for Kids {
    fn alive(&self, _agent: &yi_types::plan::doc::AgentId) -> Option<bool> {
        Some(self.alive.load(std::sync::atomic::Ordering::SeqCst))
    }
}

/// The real engine over a scratch store and workspace, behind `plan.op`, with `rlm.wait`
/// answering for the stub child; one service per call, so a second call is a new kernel.
struct PlanRig {
    dir: Scratch,
    store: PlanStore,
    engine: Arc<PlanEngine>,
    kids: Arc<Kids>,
}

impl PlanRig {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(name)?;
        let (plans, workspace) = (dir.join("plans"), dir.join("ws"));
        std::fs::create_dir_all(&workspace)?;
        let store = PlanStore::open(plans.clone())?;
        let kids = Arc::new(Kids::default());
        let resolver =
            yi_runtime::fetch::Resolver::new(workspace.clone(), yi_runtime::Wall::default())
                .with_plans_dir(plans);
        let engine = PlanEngine::new(store.clone(), kids.clone())
            .with_cwd(workspace)
            .with_output_resolve(Arc::new(resolver))
            .with_liveness(kids.clone());
        Ok(Self {
            dir,
            store,
            engine: Arc::new(engine),
            kids,
        })
    }

    fn kernel(&self) -> Arc<KernelService> {
        let mut registry = HostRegistry::default();
        yi_runtime::plan::request::register(Arc::clone(&self.engine), Actor::Owner, &mut registry);
        let kids = Arc::clone(&self.kids);
        let engine = Arc::clone(&self.engine);
        registry.register("rlm.wait", move |_payload| {
            let kids = Arc::clone(&kids);
            let engine = Arc::clone(&engine);
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let finished = kids.finished.load(std::sync::atomic::Ordering::SeqCst);
                // The host's finish hook, at the wait that reports the child's end.
                if finished {
                    tokio::task::spawn_blocking(move || {
                        engine.finish_child("kid", ChildExit::Completed, None, Some("{}".to_owned()))
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                }
                let state = if finished { "finished" } else { "running" };
                let reply = serde_json::json!({"cursor": 1, "changed": [], "states": {"kid": state}, "notes": {}});
                reply.as_object().cloned().ok_or_else(|| "reply".to_owned())
            })
        });
        service_with(registry)
    }
}

/// The plan section 8.2 program, with a stub delegate for the writer. `{mark}` is a file the
/// inline todo appends to, so a second execution of it is visible from outside the kernel.
const PROGRAM: &str = r#"
from yi import Plan, Writer, contract, cmd, schema
async def freeze_surface():
    with open(MARK, "a") as handle:
        handle.write("ran\n")
    return {"flags": ["--size"]}
async def declare(plan):
    freeze = await plan.todo(key="freeze", label="freeze the CLI surface", run=freeze_surface,
        accept=contract(schema({"type": "object", "required": ["flags"]}, critical=True)))
    await plan.todo(key="tests", label="write the test suite", after=[freeze],
        delegate=Writer(accept=contract(cmd("true", critical=True)), isolation=None))
"#;

impl PlanRig {
    /// Runs `PROGRAM` and then `tail` as one cell of a fresh or a given kernel.
    async fn run(
        &self,
        service: &Arc<KernelService>,
        tail: &str,
    ) -> Result<String, Box<dyn Error>> {
        let mark = self.dir.join("mark");
        let code = format!("MARK = {:?}\n{PROGRAM}\n{tail}", mark.display().to_string());
        let outcome = cell(service, &code).await?;
        Ok(format!(
            "{}\n{}",
            outcome.result.stdout, outcome.result.stderr
        ))
    }

    fn plan(&self) -> Result<yi_types::plan::doc::Plan, Box<dyn Error>> {
        Ok(self
            .store
            .read(&yi_types::plan::doc::PlanId::new("ship-logrotate-lite")?)?)
    }
}

const CREATE: &str = "plan = await Plan.create('ship logrotate-lite', request_id='create-01')\nawait declare(plan)\n";

/// Dies with the control: return the value without `submit` and the schema item has no
/// product; mint the url from the label and the fetch finds no blob behind it.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn an_inline_todo_completes_with_a_host_minted_artifact() -> TestResult {
    let rig = PlanRig::new("yi-kernel-inline-artifact")?;
    let service = rig.kernel();
    let tail = format!(
        "{CREATE}r = await plan.run(budget=4)\nprint('outcome', r.outcome, await r.status())"
    );
    let printed = rig.run(&service, &tail).await?;
    service.dispose().await;
    let plan = rig.plan()?;
    let freeze = plan.todos.first().ok_or("no todo")?;
    let yi_types::plan::doc::TodoState::Done {
        output: Some(output),
        resolution,
    } = &freeze.state
    else {
        return Err(format!(
            "the inline todo must verify and complete: {:?}\n{printed}",
            freeze.state
        )
        .into());
    };
    assert_eq!(
        *resolution,
        Some(yi_types::plan::contract::Resolution::VerifiedDone)
    );
    let product = br#"{"flags":["--size"]}"#;
    let digest = yi_types::plan::canonical::Digest::of(product);
    assert_eq!(
        output.to_string(),
        format!("plan://{}/artifacts/{}", plan.id, digest.hex())
    );
    assert_eq!(rig.store.artifacts(&plan.id).get(&digest)?, product);
    Ok(())
}

/// The exit journey: the section 8.2 program on a real kernel, the kernel killed mid-plan, and
/// a second kernel resuming and declaring the step after it. Dies with the control: replay the
/// recorded cell, restart the running todo, or re-run the done one, and the mark file or the
/// spawn count says so; record the new cell after its append, or not at all, and the journal does.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn resume_after_a_kernel_death_reuses_results_and_replays_no_cell() -> TestResult {
    use std::sync::atomic::Ordering;
    let rig = PlanRig::new("yi-kernel-resume")?;
    let first = rig.kernel();
    let tail = format!(
        "{CREATE}r = await plan.run(budget=4)\nprint('outcome', r.outcome, await r.status())"
    );
    let printed = rig.run(&first, &tail).await?;
    assert!(
        printed.contains("outcome unresolved"),
        "the child is still running: {printed}"
    );
    first.dispose().await;
    assert_eq!(rig.kids.spawns.load(Ordering::SeqCst), 1, "{printed}");

    let records = rig.store.journal(&rig.plan()?.id).read()?.records;
    let at = |kind: &str| records.iter().position(|record| record.record.op == kind);
    assert_eq!(
        at("program"),
        Some(1),
        "the cell is recorded right behind the init it made"
    );
    assert!(at("program") < at("start"), "and before the first effect");
    let program = std::fs::read_to_string(rig.store.plan_dir(&rig.plan()?.id).join("program.py"))?;
    assert_eq!(
        program.matches("await plan.run(budget=4)").count(),
        1,
        "{program}"
    );

    rig.kids.alive.store(true, Ordering::SeqCst);
    rig.kids.finished.store(true, Ordering::SeqCst);
    let second = rig.kernel();
    // The engine completes `tests` from its child's finish, so the owner's own op in this cell
    // is declaring and running `package`, an inline step after it.
    let tail = "plan = await Plan.resume('ship-logrotate-lite')\nawait declare(plan)\nprint('unresolved', plan.unresolved)\nasync def package():\n    return {'tarball': 'logrotate-lite.tgz'}\nawait plan.todo(key='package', after=['tests'], run=package, accept=contract(schema({'type': 'object', 'required': ['tarball']}, critical=True)))\nr = await plan.run(budget=20)\nprint('outcome', r.outcome, await r.status())";
    let printed = rig.run(&second, tail).await?;
    second.dispose().await;
    assert!(
        printed.contains("unresolved []"),
        "a live child is reconnected: {printed}"
    );
    assert!(printed.contains("outcome verified_success"), "{printed}");
    assert_eq!(
        rig.kids.spawns.load(Ordering::SeqCst),
        1,
        "resume spawns nothing again"
    );
    assert_eq!(
        std::fs::read_to_string(rig.dir.join("mark"))?,
        "ran\n",
        "the done todo ran once"
    );
    assert!(rig.plan()?.finished());
    let program = std::fs::read_to_string(rig.store.plan_dir(&rig.plan()?.id).join("program.py"))?;
    assert_eq!(program.matches("# --- cell ").count(), 2, "{program}");
    let records = rig.store.journal(&rig.plan()?.id).read()?.records;
    let last = |kind: &str| records.iter().rposition(|record| record.record.op == kind);
    assert!(
        last("program") < last("append"),
        "the resumed cell is recorded before its first effect"
    );
    Ok(())
}

/// A delegate whose children are named for their todos: the names it spawned, and those it
/// holds from spawn to reap, as `SubagentHost::holds` does behind `SessionDelegate`.
#[derive(Default)]
struct Crew(
    std::sync::Mutex<Vec<String>>,
    std::sync::Mutex<std::collections::BTreeSet<String>>,
);

impl Delegate for Crew {
    fn spawn(
        &self,
        at: &yi_types::plan::doc::TodoAddr,
        _delegation: &yi_types::plan::doc::Delegation,
    ) -> Result<yi_types::plan::doc::AgentId, String> {
        let slug = yi_types::plan::doc::PlanId::slug(at.todo.as_str());
        let name = slug.map_err(|error| error.to_string())?.to_string();
        self.0.lock().map_err(|_| "poisoned")?.push(name.clone());
        self.1.lock().map_err(|_| "poisoned")?.insert(name.clone());
        yi_types::plan::doc::AgentId::new(name).map_err(|error| error.to_string())
    }

    fn reap(
        &self,
        agent: &yi_types::plan::doc::AgentId,
        _supplied: &[yi_types::url::Url],
    ) -> Result<Option<yi_types::url::Url>, String> {
        self.1
            .lock()
            .map_err(|_| "poisoned")?
            .remove(agent.as_str());
        Ok(None)
    }
}

/// `SessionDelegate`'s liveness (`plan/dispatch.rs`): a held child is alive, finished or not.
impl yi_runtime::plan::recovery::Liveness for Crew {
    fn alive(&self, agent: &yi_types::plan::doc::AgentId) -> Option<bool> {
        Some(self.1.lock().ok()?.contains(agent.as_str()))
    }
}

/// The real engine at width one behind `plan.op`, over a `Crew` whose children all finish
/// (a name starting `fails` fails) and answer what `said` gives their name, handed to the
/// engine's finish at the wait that reports them, as the host's finish hook does.
fn crewed(
    plans: PathBuf,
    workspace: PathBuf,
    fails: &'static str,
    said: fn(&str) -> serde_json::Value,
) -> Result<(Arc<KernelService>, PlanStore), Box<dyn Error>> {
    let crew = Arc::new(Crew::default());
    let resolver = Arc::new(
        yi_runtime::fetch::Resolver::new(workspace.clone(), yi_runtime::Wall::default())
            .with_plans_dir(plans.clone()),
    );
    let store = PlanStore::open(plans)?;
    let engine = Arc::new(
        PlanEngine::new(store.clone(), crew.clone())
            .with_cwd(workspace)
            .with_output_resolve(resolver.clone())
            .with_liveness(crew.clone())
            .with_width(std::num::NonZeroUsize::MIN),
    );
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(Arc::clone(&engine), Actor::Owner, &mut registry);
    let reply = |value: serde_json::Value| value.as_object().cloned().ok_or("reply".to_owned());
    registry.register("rlm.wait", move |_payload| {
        let spawned = Arc::clone(&crew);
        let engine = Arc::clone(&engine);
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let names = spawned.0.lock().map_err(|_| "poisoned")?.clone();
            let ended = names.clone();
            tokio::task::spawn_blocking(move || {
                for name in ended {
                    let (exit, error) = if name.starts_with(fails) {
                        (ChildExit::Interrupted, Some("never came back".to_owned()))
                    } else {
                        (ChildExit::Completed, None)
                    };
                    let answer = Some(said(&name).to_string());
                    let _told_the_owner = engine.finish_child(&name, exit, error, answer);
                }
            })
            .await
            .map_err(|error| error.to_string())?;
            let states: serde_json::Map<_, _> = names
                .into_iter()
                .map(|name| {
                    let state = if name.starts_with(fails) {
                        "failed"
                    } else {
                        "finished"
                    };
                    (name, serde_json::json!(state))
                })
                .collect();
            reply(serde_json::json!({"cursor": 1, "changed": [], "states": states, "notes": {}}))
        })
    });
    registry.register("rlm.result", move |payload| {
        Box::pin(async move {
            let name = payload.get("target").and_then(serde_json::Value::as_str);
            reply(serde_json::json!({"text": "", "json": said(name.unwrap_or_default())}))
        })
    });
    registry.register("fetch", move |payload| {
        let resolver = Arc::clone(&resolver);
        Box::pin(async move {
            let url = payload.get("url").and_then(serde_json::Value::as_str);
            let url: yi_types::url::Url = url.ok_or("url")?.parse().map_err(|_| "url")?;
            let fetched = resolver.fetch(&url).map_err(|error| error.to_string())?;
            reply(serde_json::json!({"text": fetched.text}))
        })
    });
    Ok((service_with(registry), store))
}

/// Both shapes in one cell: three readers under fork_join, then a scatter whose lead asks twice.
const SHAPES: &str = r#"
from yi import Plan, Reader, contract, schema, fork_join, scatter, shapes
answer = contract(schema(shapes.ANSWER, critical=True))
plan = await Plan.create("fork then join", request_id="fork")
for key in ("a", "b", "c"):
    await plan.todo(key=key, delegate=Reader(partition=[f"local://docs/{key}.md"]), accept=answer)
r = await plan.run(shape=fork_join, budget=20)
print("fork_join", r.outcome, r.refusals)
plan = await Plan.create("which module rotates by size", request_id="scatter")
for key in ("api", "cli", "web"):
    await plan.todo(key=key, delegate=Reader(partition=[f"local://docs/{key}.md"]), accept=answer)
async def lead(answers, number):
    print("round", number, [(a["reader"], [q["text"] for q in a["quotes"]]) for a in answers])
    return {"ask": "and by age?"} if number == 1 else {"commit": answers[0]["answer"]}
await plan.todo(key="lead", run=lead, accept=contract(schema({"type": "object", "required": ["answer"]}, critical=True)))
r = await plan.run(shape=scatter, budget=20)
print("scatter", r.outcome, r.refusals, [(t.key, t._doc["state"]) for t in plan.todos])
"#;

/// The F1b journey: the library's shapes against the real `admit`, step table and verifier.
/// Dies with the control: start past a refused todo and the width-one engine refuses again, out
/// of order; hand the lead an unverified or out-of-partition quote and round one names `cli`;
/// leave a failed reader failed and the plan cannot reach `verified_success`.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn both_shapes_schedule_under_the_real_admission_and_step_table() -> TestResult {
    let dir = Scratch::new("yi-kernel-shapes")?;
    let (plans, workspace) = (dir.join("plans"), dir.join("ws"));
    std::fs::create_dir_all(workspace.join("docs"))?;
    std::fs::write(workspace.join("docs/api.md"), "usage\nrotate(size)\n")?;
    std::fs::write(workspace.join("docs/cli.md"), "flags\n--age DAYS\n")?;
    // The `web` readers never come back, so the shape settles a failed reader for real. A `cli`
    // reader cites a line its own page does not have, and one that is right but sits in the
    // `api` reader's partition; both go before the lead sees the answer.
    let (service, store) = crewed(plans, workspace, "web", |name| {
        let good =
            serde_json::json!({"url": "local://docs/api.md", "line": 2, "text": "rotate(size)"});
        let quotes = if name.starts_with("cli") {
            serde_json::json!([{"url": "local://docs/cli.md", "line": 1, "text": "--age DAYS"}, good])
        } else {
            serde_json::json!([good])
        };
        serde_json::json!({"answer": "rotate()", "quotes": quotes})
    })?;
    let outcome = cell(&service, SHAPES).await?;
    service.dispose().await;
    let printed = format!("{}\n{:?}", outcome.result.stdout, outcome.result.error);
    assert!(
        printed.contains("fork_join verified_success {}"),
        "{printed}"
    );
    let fork = store.journal(&yi_types::plan::doc::PlanId::slug("fork then join")?);
    let starts: Vec<String> = fork
        .read()?
        .records
        .iter()
        .filter(|record| record.record.op == "start")
        .filter_map(|record| record.record.todo.as_ref().map(ToString::to_string))
        .collect();
    assert_eq!(
        starts,
        ["a", "b", "c"],
        "one slot: each starts as the one before it is accepted, and a held one is never tried"
    );
    assert!(
        printed.contains("round 1 [('api', ['rotate(size)'])]"),
        "{printed}"
    );
    assert!(
        printed.contains("round 2 [('api-r2', ['rotate(size)'])]"),
        "{printed}"
    );
    assert!(printed.contains("scatter verified_success {}"), "{printed}");
    assert!(
        printed.contains("('web', 'abandoned')") && printed.contains("('web-r2', 'abandoned')"),
        "a failed reader is retried and dropped on the real step table: {printed}"
    );
    Ok(())
}

/// A pod whose readers block and whose command is green, then one whose readers are clean and
/// whose command is red, over `fixtures/plans/worktree/repo.sh`.
const POD: &str = r#"
from yi import Plan, Writer, cmd, contract, review_pod
from yi.recipes.review_pod import declare
for goal, check in (("review the launcher", "grep -q 'subcommands: list' rotate.sh"), ("review it again", "grep -q compress rotate.sh")):
    plan = await Plan.create(goal, request_id=goal.replace(" ", "-"))
    await declare(plan, ["local://rotate.sh"], cmd(check, critical=True), arbiter=Writer(accept=contract(cmd(check, critical=True)), isolation=None))
    r = await plan.run(shape=review_pod, budget=20)
    print(goal, r.outcome, [(t.key, t._doc["state"]) for t in plan.todos])
    print(plan["arbiter-r2"]._doc["delegation"]["note"])
"#;

/// The F3b journey: the pod recipe against the real step table and the real `cmd` verifier.
/// Dies with the control: let a finding vote and the first pod fails or the second passes; skip
/// the quote seam and the invented finding is in the arbiter's note.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn review_pod_on_the_fixture_repo() -> TestResult {
    let dir = Scratch::new("yi-kernel-pod")?;
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/worktree/repo.sh");
    let built = yi_tools::command("sh")
        .arg(script)
        .arg(dir.join("repo"))
        .output()?;
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let (service, store) = crewed(dir.join("plans"), dir.join("repo/parent"), "-", |name| {
        let quote = |line: u32, text: &str| serde_json::json!([{"url": "local://rotate.sh", "line": line, "text": text}]);
        match name {
            "read-correctness" => {
                serde_json::json!({"answer": "BLOCKER: the launcher is a stub", "quotes": quote(4, "the launcher is a stub")})
            }
            "read-tests" => {
                serde_json::json!({"answer": "BLOCKER: nothing is tested", "quotes": quote(1, "def test_")})
            }
            _ => serde_json::json!({"answer": null, "quotes": []}),
        }
    })?;
    let outcome = cell(&service, POD).await?;
    service.dispose().await;
    let printed = format!("{}\n{:?}", outcome.result.stdout, outcome.result.error);
    assert!(
        printed.contains("review the launcher verified_success"),
        "{printed}"
    );
    assert!(printed.contains("review it again failed"), "{printed}");
    assert!(
        printed.contains("read-correctness: BLOCKER: the launcher is a stub [local://rotate.sh:4]")
            && printed.contains("read-tests: no backed finding")
            && !printed.contains("nothing is tested"),
        "{printed}"
    );
    let plan = store.read(&yi_types::plan::doc::PlanId::slug("review the launcher")?)?;
    use yi_types::plan::doc::TodoStateName::{Abandoned, Done};
    let states: Vec<_> = plan
        .todos
        .iter()
        .map(|todo| yi_types::plan::doc::TodoStateName::of(&todo.state))
        .collect();
    assert_eq!(
        states,
        [Done, Done, Done, Abandoned, Done],
        "the declared arbiter ran as its issue"
    );
    Ok(())
}
