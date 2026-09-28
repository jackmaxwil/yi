//! The node card, kernel admission and container placement (D285, D286), on the files and
//! processes a user's machine would see.

use crate::scratch::Scratch;
use crate::support;

use std::error::Error;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_runtime::node;
use yi_tools::CancelFlag;
use yi_types::config::NodeConfig;

type TestResult = Result<(), Box<dyn Error>>;

const HOLDER_HOME: &str = "NODE_HOLDER_HOME";

fn write_card(home: &Path, slots: u8, isolation: &[&str]) -> TestResult {
    let config = NodeConfig {
        slots: NonZeroU8::new(slots),
        isolation: Some(isolation.iter().map(|kind| (*kind).to_owned()).collect()),
    };
    let card = node::overridden(node::computed(4, 16, false, "test".to_owned()), &config);
    std::fs::create_dir_all(home.join(".yi"))?;
    std::fs::write(home.join(".yi/node.json"), serde_json::to_vec(&card)?)?;
    Ok(())
}

fn never() -> CancelFlag {
    Arc::new(|| false)
}

fn at_once() -> CancelFlag {
    Arc::new(|| true)
}

#[test]
fn a_card_is_computed_once_and_an_edit_to_it_sticks() -> TestResult {
    let slots = |cpus| node::computed(cpus, 8, false, "n".to_owned()).slots.get();
    assert_eq!([slots(1), slots(2), slots(4), slots(16)], [1, 1, 3, 8]);
    let docker = node::computed(4, 8, true, "n".to_owned());
    assert_eq!(docker.isolation, ["worktree", "container"]);

    let dir = Scratch::new("yi-node-card")?;
    let card = node::card(&dir)?;
    let cores = std::thread::available_parallelism()?.get();
    assert_eq!(
        usize::from(card.slots.get()),
        cores.saturating_sub(1).clamp(1, 8)
    );
    assert_eq!(card.isolation.first().map(String::as_str), Some("worktree"));
    let written: Value = serde_json::from_slice(&std::fs::read(dir.join(".yi/node.json"))?)?;
    assert_eq!(
        written["slots"],
        card.slots.get(),
        "the card is on disk: {written}"
    );
    assert!(
        written["capacity"]["cpus"]
            .as_u64()
            .is_some_and(|cpus| cpus >= 1)
    );

    write_card(&dir, 3, &["worktree"])?;
    assert_eq!(
        node::card(&dir)?.slots.get(),
        3,
        "an edit to node.json is read, not recomputed"
    );
    std::fs::write(dir.join(".yi/node.json"), r#"{"slots": 0}"#)?;
    let broken = node::card(&dir).err().ok_or("a broken card loaded")?;
    assert!(broken.contains("node.json"), "{broken}");
    Ok(())
}

#[test]
fn the_config_overrides_the_card_field_by_field() -> TestResult {
    let card = node::computed(16, 64, true, "laptop".to_owned());
    let config = NodeConfig {
        slots: NonZeroU8::new(2),
        isolation: None,
    };
    let overridden = node::overridden(card.clone(), &config);
    assert_eq!(overridden.slots.get(), 2);
    assert_eq!(
        overridden.isolation, card.isolation,
        "an unset key keeps the card's"
    );
    assert_eq!(overridden.name, "laptop");
    let parsed: yi_types::config::UserConfig =
        serde_json::from_str(r#"{"node": {"slots": 3, "isolation": ["worktree"]}}"#)?;
    let node = parsed.node.ok_or("node config")?;
    assert_eq!(node::overridden(card, &node).isolation, ["worktree"]);
    let zero = serde_json::from_str::<yi_types::config::UserConfig>(r#"{"node": {"slots": 0}}"#);
    assert!(
        zero.is_err(),
        "zero slots is refused at the parse, never a node that admits nothing"
    );
    Ok(())
}

#[tokio::test]
async fn a_third_kernel_waits_for_a_slot_and_proceeds_when_one_exits() -> TestResult {
    let dir = Scratch::new("yi-node-admit")?;
    write_card(&dir, 2, &["worktree"])?;
    let quiet = |_: &str| {};
    let first = node::admit(&dir, "pid 1 first", None, &quiet, &never()).await?;
    let second = node::admit(&dir, "pid 2 second", None, &quiet, &never()).await?;
    assert!(first.waited.is_none() && second.waited.is_none());

    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let home = dir.to_path_buf();
    let third = tokio::spawn(async move {
        let progress = move |text: &str| {
            if let Ok(mut seen) = sink.lock() {
                seen.push(text.to_owned());
            }
        };
        node::admit(&home, "pid 3 third", None, &progress, &never()).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        !third.is_finished(),
        "the third boot must wait while both slots are held"
    );
    let told = seen.lock().map_err(|_| "poisoned")?.clone();
    let notice = told.first().ok_or("the wait said nothing")?;
    assert!(
        notice.starts_with("node: 2 of 2 slots held (node.slots=2); this kernel waits for one"),
        "{notice}"
    );
    assert!(
        notice.contains("pid 1 first") && notice.contains("pid 2 second"),
        "the notice names what holds the slots: {notice}"
    );
    assert_eq!(
        told.len(),
        1,
        "one notice per change in holders, not per poll: {told:?}"
    );

    drop(first);
    let admitted = tokio::time::timeout(std::time::Duration::from_secs(10), third).await???;
    let waited = admitted
        .waited
        .ok_or("the admitted call is told it waited")?;
    assert!(
        waited.contains("It waited") && waited.contains("slot 0"),
        "{waited}"
    );
    let home = dir.to_path_buf();
    let fourth =
        tokio::spawn(
            async move { node::admit(&home, "pid 4", None, &|_: &str| {}, &never()).await },
        );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        !fourth.is_finished(),
        "the third holds slot 0, so the node is full again"
    );
    write_card(&dir, 3, &["worktree"])?;
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), fourth).await??;
    assert!(
        raised.is_ok_and(|admitted| admitted.slot.is_some()),
        "the notice's remedy, a higher slots in node.json, admits the waiting boot"
    );
    Ok(())
}

#[test]
fn a_crashed_holders_slot_is_reclaimed() -> TestResult {
    let dir = Scratch::new("yi-node-crash")?;
    write_card(&dir, 1, &["worktree"])?;
    let mut child = yi_tools::command(std::env::current_exe()?)
        .args(["--exact", "node::node_holder_child", "--nocapture"])
        .env(HOLDER_HOME, &*dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !dir.join("held").is_file() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err("the holder never took its slot".into());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("the holder exited before holding: {status}").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    let quiet = |_: &str| {};
    let full = runtime
        .block_on(node::admit(&dir, "pid parent", None, &quiet, &at_once()))
        .err()
        .ok_or("the slot was free while the holder lived")?;
    assert!(full.contains(&format!("pid {}", child.id())), "{full}");
    child.kill()?;
    child.wait()?;
    let reclaimed = runtime.block_on(node::admit(&dir, "pid parent", None, &quiet, &at_once()));
    assert!(
        reclaimed.is_ok(),
        "a killed holder's slot is free: {:?}",
        reclaimed.err()
    );
    Ok(())
}

#[tokio::test]
async fn a_kernel_whose_family_holds_a_slot_never_waits_and_another_family_does() -> TestResult {
    let dir = Scratch::new("yi-node-family")?;
    write_card(&dir, 1, &["worktree"])?;
    let (quiet, ours, theirs) = (|_: &str| {}, dir.join("family-a"), dir.join("family-b"));
    let parent = node::admit(&dir, "pid 1 parent", Some(&ours), &quiet, &never()).await?;
    assert!(parent.slot.is_some(), "a free slot is taken");
    let child = node::admit(&dir, "pid 1 child", Some(&ours), &quiet, &at_once()).await;
    let child = child.map_err(|full| format!("the child waited on its own family: {full}"))?;
    assert!(child.slot.is_none(), "the child shares its family's slot");
    for (family, who) in [
        (Some(theirs.as_path()), "another family"),
        (None, "no family"),
    ] {
        let refused = node::admit(&dir, "pid 2", family, &quiet, &at_once()).await;
        let full = refused
            .err()
            .ok_or(format!("{who} rode a slot it does not hold"))?;
        assert!(full.contains("pid 1 parent"), "{full}");
    }
    Ok(())
}

/// The holder the crash test kills: takes the node's one slot, says so, and waits to die.
#[test]
fn node_holder_child() -> TestResult {
    let Some(home) = std::env::var_os(HOLDER_HOME) else {
        return Ok(());
    };
    let home = std::path::PathBuf::from(home);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    let holder = format!("pid {}", std::process::id());
    let _held = runtime.block_on(node::admit(&home, &holder, None, &|_: &str| {}, &never()))?;
    std::fs::write(home.join("held"), "")?;
    std::thread::sleep(std::time::Duration::from_secs(120));
    Ok(())
}

fn kwargs(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

#[tokio::test]
async fn a_container_spawn_on_a_node_without_docker_is_refused_plainly() -> TestResult {
    let root = Scratch::new("yi-node-refuse")?;
    let store = support::memory_store("node-refuse");
    let family = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    write_card(&root.join("home"), 2, &["worktree"])?;
    let refused = family
        .host
        .spawn(
            "box".to_owned(),
            kwargs(&[("isolation", "container:alpine")]),
        )
        .err()
        .ok_or("a container spawn ran on a node without docker")?;
    assert!(
        refused
            .starts_with("container:alpine needs docker, and this node has no container isolation"),
        "{refused}"
    );
    for bad in ["container:", "container:-v", "container:a b"] {
        let refused = family
            .host
            .spawn("box".to_owned(), kwargs(&[("isolation", bad)]))
            .err()
            .ok_or("a bad image was admitted")?;
        assert!(refused.contains("names no image"), "{bad}: {refused}");
    }
    Ok(())
}

#[tokio::test]
async fn a_plan_delegation_asks_for_a_container_the_way_rlm_run_does() -> TestResult {
    use yi_runtime::plan::ops::Delegate;
    use yi_types::plan::doc::{Delegation, PlanId, TodoAddr, TodoLabel};
    let root = Scratch::new("yi-node-plan")?;
    let store = support::memory_store("node-plan");
    let family = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    write_card(&root.join("home"), 2, &["worktree"])?;
    let delegate = yi_runtime::plan::dispatch::SessionDelegate::new(
        Arc::clone(&family.host),
        Arc::new(|_message, _mode| {}),
        Arc::new(yi_runtime::fetch::FetchLog::new()),
    );
    let at = TodoAddr {
        plan: PlanId::slug("placed")?,
        todo: TodoLabel::new("build it")?,
    };
    let spec = |isolation: &str| serde_json::json!({"role": "writer", "isolation": isolation});
    let delegation = |isolation: &str| {
        serde_json::from_value::<Delegation>(
            serde_json::json!({"spec": spec(isolation), "accept": "stated: built"}),
        )
    };
    let refused = delegate
        .spawn(&at, &delegation("container:rust:1.91")?)
        .err();
    let refused = refused.ok_or("a container todo ran on a node without docker")?;
    assert!(
        refused.starts_with("container:rust:1.91 needs docker"),
        "{refused}"
    );
    let unknown = delegate
        .spawn(&at, &delegation("vm:big")?)
        .err()
        .ok_or("vm:big spawned")?;
    assert!(unknown.contains("container:<image>"), "{unknown}");
    let uncontracted = serde_json::from_value::<yi_types::plan::op::TodoSpec>(serde_json::json!({
        "label": "build it",
        "delegation": {"spec": spec("container:rust:1.91"), "accept": "stated: built"},
    }));
    assert!(
        uncontracted.is_err_and(|error| error.to_string().contains("contract")),
        "a container todo owes a contract as a worktree todo does"
    );
    Ok(())
}

fn docker(args: &[&str]) -> Result<String, String> {
    let output = yi_tools::command("docker")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

fn docker_here(test: &str) -> bool {
    match docker(&["version", "--format", "{{.Server.Version}}"]) {
        Ok(_) => true,
        Err(reason) => {
            eprintln!(
                "SKIP {test}: docker does not answer here ({reason}); the container path ran nowhere"
            );
            false
        }
    }
}

fn git_repo(label: &str) -> Result<Scratch, Box<dyn Error>> {
    let repo = Scratch::new(label)?;
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.invalid"],
        &["config", "user.name", "Yi Test"],
        &["commit", "-q", "--allow-empty", "-m", "base"],
    ] {
        let status = yi_tools::command("git")
            .current_dir(&repo)
            .args(args)
            .status()?;
        if !status.success() {
            return Err(format!("git {args:?} failed").into());
        }
    }
    Ok(repo)
}

fn running(name: &str) -> Result<usize, String> {
    let filter = format!("name=^/{name}$");
    Ok(docker(&["ps", "-aq", "--filter", &filter])?.lines().count())
}

#[tokio::test]
async fn a_container_child_runs_bash_in_its_container_and_merges_like_a_worktree() -> TestResult {
    if !docker_here("a_container_child_runs_bash_in_its_container") {
        return Ok(());
    }
    let repo = git_repo("yi-node-box")?;
    let root = Scratch::new("yi-node-box-root")?;
    let store = support::memory_store("node-box");
    // The hook and the pointer are what a child would plant to run code under the host's git.
    let hold = "uname -a > container.txt && cat /etc/alpine-release >> container.txt; \
        gd=$(sed -n 's/^gitdir: //p' .git); (echo '#!/bin/sh' > \"$gd/../../hooks/post-merge\"); \
        (echo gitdir: /tmp > .git); true";
    let family = support::family(root.to_path_buf(), repo.to_path_buf(), store, Some(hold));
    family.host.spawn(
        "box".to_owned(),
        kwargs(&[("name", "boxed"), ("isolation", "container:alpine:3.20")]),
    )?;
    let name = family
        .built
        .lock()
        .map_err(|_| "poisoned")?
        .first()
        .and_then(|built| built.wall.container.clone())
        .ok_or("the child was built without its container on the wall")?;
    assert!(
        family.reaches("boxed", "finished").await,
        "{:?}",
        family.state_of("boxed")
    );
    assert_eq!(
        running(&name)?,
        1,
        "the container lives as long as the child's record"
    );

    assert!(
        !repo.join(".git/hooks/post-merge").exists(),
        "the container wrote a hook the host's next git runs"
    );
    let merged = family.host.merge_worktree("boxed")?;
    assert_eq!(merged["merged"], true, "{merged:?}");
    let landed = std::fs::read_to_string(repo.join("container.txt"))?;
    assert!(
        landed.starts_with("Linux "),
        "uname ran in the container: {landed}"
    );
    assert!(
        landed
            .lines()
            .nth(1)
            .is_some_and(|line| line.starts_with('3')),
        "the file came from the alpine image, not the host: {landed}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(repo.join("container.txt"))?.uid();
        assert_eq!(
            owner,
            std::fs::metadata(&*repo)?.uid(),
            "written as the owner, not root"
        );
    }
    family.host.delete("boxed")?;
    assert_eq!(running(&name)?, 0, "the reap removes the container");
    Ok(())
}

/// The scratch home shares the real home's kernel venv, so a test boots kernels, not a venv.
fn link_kernel_venv(home: &Path) -> TestResult {
    let real = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?);
    let venv = yi_kernel::bootstrap::kernel_venv_dir(&real);
    if venv.is_dir() {
        std::fs::create_dir_all(home.join(".yi"))?;
        std::os::unix::fs::symlink(&venv, yi_kernel::bootstrap::kernel_venv_dir(home))?;
    }
    Ok(())
}

#[test]
fn a_timed_out_container_call_leaves_nothing_running_in_its_container() -> TestResult {
    use yi_tools::Tool;
    if !docker_here("a_timed_out_container_call_leaves_nothing_running") {
        return Ok(());
    }
    let lane = Scratch::new("yi-node-sweep")?;
    let container = node::Container::up("alpine:3.20", &lane, &|_: &str| {})?;
    let mut context = yi_tools::ToolContext::new(lane.to_path_buf());
    context.container = Some(container.name().to_owned());
    let call = serde_json::json!({"command": "sleep 297 & sleep 298", "timeout_secs": 2});
    let call = call.as_object().cloned().ok_or("an object")?;
    let output = yi_tools::BashTool::default().execute(call, &context);
    assert!(output.is_error, "the call timed out: {:?}", output.result);
    let left = docker(&["exec", container.name(), "ps", "-o", "args"])?;
    assert!(
        !left.contains("sleep 29"),
        "the killed call still runs in its container: {left}"
    );
    Ok(())
}

struct Demo {
    host: Arc<yi_runtime::SubagentHost>,
    trees: Arc<Mutex<Vec<std::path::PathBuf>>>,
}

fn kernel_in(
    home: &Path,
    cwd: &Path,
    family: Option<&Path>,
    on_boot: Option<Arc<yi_runtime::kernel::BootFn>>,
) -> Arc<yi_runtime::KernelService> {
    let mut registry = yi_runtime::HostRegistry::default();
    registry.register_mcp_stubs();
    Arc::new(yi_runtime::KernelService::new(
        yi_runtime::KernelServiceOptions {
            cwd: cwd.to_path_buf(),
            home: home.to_path_buf(),
            session_dir: None,
            family_dir: family.map(Path::to_path_buf),
            host: Arc::new(registry),
            on_restore: None,
            on_boot,
            sandbox: None,
            snapshot_key: None,
            cell_ceiling: None,
            per_session_state: false,
        },
    ))
}

/// Each child boots its kernel with `cell`, then writes `placed.txt` through bash.
fn demo_family(root: &Path, repo: &Path, home: &Path, cell: String) -> Demo {
    use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let trees: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let seen = Arc::clone(&trees);
    let kernel_home = home.to_path_buf();
    let family = root.join("family");
    let (events, _keep) = tokio::sync::broadcast::channel(256);
    let host = Arc::new(yi_runtime::SubagentHost::new(
        yi_runtime::SubagentHostOptions {
            provider: Arc::new(yi_runtime::ProviderStream::new(None)),
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: root.join("children"),
            plans_dir: repo.join(".yi/plans"),
            cwd: repo.to_path_buf(),
            home: home.to_path_buf(),
            lane_slots: 9,
            defaults: Arc::new(|| (support::faux_model(), yi_types::model::Effort::Medium)),
            factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
                let cwd = build
                    .cwd
                    .map(Path::to_path_buf)
                    .ok_or("a container child has a lane")?;
                if let Ok(mut seen) = seen.lock() {
                    seen.push(cwd.clone());
                }
                let call = |id: &str, tool: &str, key: &str, text: &str| {
                    let mut args = Map::new();
                    args.insert(key.to_owned(), Value::String(text.to_owned()));
                    faux_assistant_message(
                        vec![faux_tool_call(id, tool, args)],
                        StopReason::ToolUse,
                    )
                };
                let provider = Arc::new(yi_runtime::ProviderStream::new(None));
                provider.queue_faux(vec![
                    call("c1", "ipython", "code", &cell),
                    call("c2", "bash", "command", "uname -s > placed.txt"),
                    faux_assistant_message(vec![faux_text("placed")], StopReason::Stop),
                ]);
                let config = yi_runtime::SessionConfig {
                    system_prompt: "child sys".to_owned(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: yi_loop::ExecutionMode::Sequential,
                };
                let mut child = yi_runtime::AgentSession::new(config, provider);
                let service = kernel_in(&kernel_home, &cwd, Some(&family), None);
                child.set_kernel_service(Arc::clone(&service));
                child.set_wall(build.wall.clone());
                let mut tools = yi_tools::builtin_tools();
                tools.push(yi_runtime::kernel::ipython_tool(service));
                child.use_tools(tools, cwd, None);
                Ok(child)
            }),
            notice: Arc::new(|_, _| {}),
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(|_, _| {}),
            attribute: Arc::new(|_| {}),
            store: Arc::new(|| None),
            family_live: Arc::new(|| 0),
        },
    ));
    Demo { host, trees }
}

/// The stage-5 demo: eight container children hold a laptop's eight slots with their kernels,
/// a ninth kernel waits and says who holds them, and a reap lets it in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "tier-2 journey: `just journeys`"]
async fn eight_container_children_fill_the_node_and_a_ninth_kernel_waits() -> TestResult {
    if !docker_here("eight_container_children_fill_the_node") {
        return Ok(());
    }
    let repo = git_repo("yi-node-demo")?;
    let root = Scratch::new("yi-node-demo-root")?;
    let home = root.join("home");
    write_card(&home, 8, &["worktree", "container"])?;
    link_kernel_venv(&home)?;
    let demo = demo_family(
        &root,
        &repo,
        &home,
        "import os\nprint(os.getpid())".to_owned(),
    );
    for index in 0..8 {
        let name = format!("box-{index}");
        let asked = kwargs(&[("name", &name), ("isolation", "container:alpine:3.20")]);
        demo.host.spawn("place a file".to_owned(), asked)?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let trees = || {
        demo.trees
            .lock()
            .map(|trees| trees.clone())
            .unwrap_or_default()
    };
    while trees().len() < 8 || !trees().iter().all(|tree| tree.join("placed.txt").is_file()) {
        if std::time::Instant::now() > deadline {
            return Err("the eight children never all placed their file".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    for tree in trees() {
        let placed = std::fs::read_to_string(tree.join("placed.txt"))?;
        assert_eq!(placed.trim(), "Linux", "bash ran in the child's container");
    }

    let told: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&told);
    let on_boot: Arc<yi_runtime::kernel::BootFn> = Arc::new(move |step: Option<&str>| {
        if let (Some(step), Ok(mut told)) = (step, sink.lock()) {
            told.push(step.to_owned());
        }
    });
    let ninth = kernel_in(&home, &repo, None, Some(on_boot));
    let cell = tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        yi_tools::KernelBridge::execute_cell(ninth.as_ref(), "print(9)", &cancelled)
    });
    let waiting = |told: &[String]| {
        told.iter().any(|step| {
            step.starts_with("node: 8 of 8 slots held (node.slots=8); this kernel waits for one")
        })
    };
    while !waiting(&told.lock().map_err(|_| "poisoned")?) {
        if cell.is_finished() || std::time::Instant::now() > deadline {
            return Err(format!("the ninth kernel never waited: {:?}", told.lock().ok()).into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(
        !cell.is_finished(),
        "the ninth kernel boots only once a slot frees"
    );

    let merged = demo.host.merge_worktree("box-0")?;
    assert_eq!(merged["merged"], true, "{merged:?}");
    assert_eq!(
        std::fs::read_to_string(repo.join("placed.txt"))?.trim(),
        "Linux"
    );
    demo.host.delete("box-0")?;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), cell).await???;
    assert_eq!(outcome.result.stdout.trim(), "9");
    assert!(
        outcome.notes.iter().any(|note| note.contains("It waited")),
        "the call that waited is told so: {:?}",
        outcome.notes
    );
    for index in 1..8 {
        demo.host.delete(&format!("box-{index}")).ok();
    }
    Ok(())
}

/// The deadlock D285 first shipped with: one slot, held by a parent kernel whose cell waits
/// on a child that boots a kernel of its own. The family's slot admits the child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parent_cell_waiting_on_its_childs_kernel_completes_on_one_slot() -> TestResult {
    let repo = git_repo("yi-node-family-e2e")?;
    let root = Scratch::new("yi-node-family-root")?;
    let home = root.join("home");
    write_card(&home, 1, &["worktree"])?;
    link_kernel_venv(&home)?;
    let (up, booted) = (root.join("parent-up"), root.join("child-booted"));
    let child_cell = format!(
        "open({:?}, 'w').write('booted')",
        booted.display().to_string()
    );
    let demo = demo_family(&root, &repo, &home, child_cell);
    let parent = kernel_in(&home, &repo, Some(&root.join("family")), None);
    let parent_cell = format!(
        "import os, time\nopen({up:?}, 'w').close()\ndeadline = time.time() + 90\nwhile not os.path.exists({booted:?}) and time.time() < deadline:\n    time.sleep(0.1)\nprint('child booted' if os.path.exists({booted:?}) else 'child never booted')",
        up = up.display().to_string(),
        booted = booted.display().to_string(),
    );
    let cell = tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        yi_tools::KernelBridge::execute_cell(parent.as_ref(), &parent_cell, &cancelled)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while !up.is_file() {
        if cell.is_finished() || std::time::Instant::now() > deadline {
            return Err("the parent kernel never booted".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let asked = kwargs(&[("name", "kid"), ("isolation", "worktree")]);
    demo.host.spawn("boot a kernel".to_owned(), asked)?;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), cell).await???;
    demo.host.delete("kid").ok();
    assert_eq!(
        outcome.result.stdout.trim(),
        "child booted",
        "the parent's cell waited out its child: {:?}",
        outcome.notes
    );
    Ok(())
}
