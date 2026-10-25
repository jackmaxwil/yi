//! One kernel, two writers. The agent's tool and a console-run cell share a session's
//! kernel, and `KernelManager::execute` holds `execution_queue` across the whole cell —
//! that lock, not anything above it, is why the second caller waits rather than landing
//! in the busy-reuse path and interrupting work that is still running.
//!
//! The kernel's worktree verbs live here too: what `rlm.merge_worktree` replies with.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
use yi_tools::CancelFlag;

use crate::scratch::Scratch;
use crate::support;

type TestResult = Result<(), Box<dyn Error>>;
type Changed = (String, u64, u64);

fn service() -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
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

fn never_cancelled() -> CancelFlag {
    Arc::new(|| false)
}

/// The cell's own stdout, which is where the ordering is legible.
fn stdout_of(output: &yi_tools::ToolOutput) -> String {
    output
        .result
        .details
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_second_cell_queues_behind_the_first_instead_of_interrupting_it() -> TestResult {
    let service = service();
    // Boot the kernel first: the wait for a cold start would otherwise land inside the
    // window this test measures.
    let warm = service
        .execute_user_cell("print('warm')", &never_cancelled())
        .await;
    assert!(!warm.is_error, "the kernel never booted: {warm:?}");

    let slow = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let out = service
                .execute_user_cell(
                    "import time\ntime.sleep(1)\nprint('slow')",
                    &never_cancelled(),
                )
                .await;
            (Instant::now(), out)
        })
    };
    // Long enough that the slow cell holds the kernel, short enough to stay inside its sleep.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let quick = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let out = service
                .execute_user_cell("print('quick')", &never_cancelled())
                .await;
            (Instant::now(), out)
        })
    };

    let (slow_at, slow_out) = slow.await?;
    let (quick_at, quick_out) = quick.await?;
    service.dispose().await;

    assert!(
        !slow_out.is_error,
        "the first cell ran to completion rather than being interrupted: {:?}",
        slow_out.result
    );
    assert!(
        !quick_out.is_error,
        "the second cell ran: {:?}",
        quick_out.result
    );
    assert!(
        stdout_of(&slow_out).contains("slow"),
        "the first cell printed: {:?}",
        stdout_of(&slow_out)
    );
    assert!(
        stdout_of(&quick_out).contains("quick"),
        "the second cell printed: {:?}",
        stdout_of(&quick_out)
    );
    assert!(
        slow_at <= quick_at,
        "the queue is first in, first out: the second cell finished first"
    );
    Ok(())
}

fn cancelled_after(delay: Duration) -> CancelFlag {
    let at = Instant::now() + delay;
    Arc::new(move || Instant::now() >= at)
}

fn text_of(output: &yi_tools::ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            yi_types::message::Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

/// The field shape: a cell polling with `await asyncio.sleep(30)`, cancelled by its caller.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_cancelled_awaiting_cell_leaves_the_namespace_standing() -> TestResult {
    let service = service();
    let set = service
        .execute_user_cell("keep = 42", &never_cancelled())
        .await;
    assert!(!set.is_error, "{:?}", set.result);
    let poll = "import asyncio\nwhile True:\n    await asyncio.sleep(30)";
    let stopped = service
        .execute_user_cell(poll, &cancelled_after(Duration::from_secs(1)))
        .await;
    assert!(stopped.is_error, "{:?}", stopped.result);
    let after = service
        .execute_user_cell("print(keep)", &never_cancelled())
        .await;
    service.dispose().await;
    assert_eq!(stdout_of(&after), "42\n", "{}", text_of(&after));
    assert_eq!(after.result.details["kernelRestarted"], false);
    Ok(())
}

/// A cell deaf to SIGINT outlives the busy window, so the kernel has to go; the state says so
/// until the next cell.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_forced_restart_names_what_it_lost_in_the_result_and_the_state() -> TestResult {
    let service = service();
    let set = service
        .execute_user_cell("keep = 42\nalso = 7", &never_cancelled())
        .await;
    assert!(!set.is_error, "{:?}", set.result);
    let deaf = "import signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\ntime.sleep(60)";
    let _ = service
        .execute_user_cell(deaf, &cancelled_after(Duration::from_secs(1)))
        .await;
    let after = service
        .execute_user_cell("print('fresh')", &never_cancelled())
        .await;
    let state = service.state();
    let _ = service.execute_user_cell("1", &never_cancelled()).await;
    let settled = service.state();
    service.dispose().await;
    let text = text_of(&after);
    assert_eq!(after.result.details["kernelRestarted"], true, "{text}");
    assert!(
        text.contains("[IPython kernel was restarted; 2 names lost: also, keep."),
        "{text}"
    );
    assert!(state.contains("restarted; 2 names lost"), "{state}");
    assert!(
        !settled.contains("restarted"),
        "a later cell left the note: {settled}"
    );
    Ok(())
}

/// The names the state line offers are checked against the live modules, not a copy of them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn the_first_cell_shows_the_rlm_table_once_and_the_state_names_the_surface() -> TestResult {
    let service = service();
    let before = service.state();
    assert!(
        before.contains("help(rlm) and help(yi) are exact"),
        "{before}"
    );
    let offered = |module: &str| -> Vec<String> {
        before
            .split(&format!("{module}: "))
            .nth(1)
            .and_then(|rest| rest.split(" · ").next())
            .map(|names| names.split(", ").map(|name| format!("{name:?}")).collect())
            .unwrap_or_default()
    };
    let (rlm, yi) = (offered("rlm"), offered("yi"));
    assert!(!rlm.is_empty() && !yi.is_empty(), "{before}");
    let check = format!(
        "import yi\nprint(all(hasattr(rlm, n) for n in [{}]) and all(hasattr(yi, n) for n in [{}]))",
        rlm.join(", "),
        yi.join(", ")
    );
    let first = service.execute_user_cell(&check, &never_cancelled()).await;
    let second = service.execute_user_cell("1", &never_cancelled()).await;
    let after = service.state();
    service.dispose().await;
    assert_eq!(stdout_of(&first), "True\n", "{}", text_of(&first));
    let table = text_of(&first);
    assert!(table.contains("[rlm, shown once per session;"), "{table}");
    assert!(table.contains("await rlm.send(target, message,"), "{table}");
    assert!(table.contains("\nrlm.bash(command)"), "{table}");
    assert!(
        !text_of(&second).contains("shown once per session"),
        "{}",
        text_of(&second)
    );
    assert!(!after.contains("help(rlm)"), "{after}");
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> TestResult {
    let out = yi_tools::command("git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into())
}

/// A repo holding `README.md` and `my notes.txt`, and a worktree child that ran `hold` in its lane
/// and finished; `in_lane` runs the parent in a linked worktree holding a commit of its own.
async fn finished_child(
    label: &str,
    hold: &'static str,
    in_lane: bool,
) -> Result<(Scratch, PathBuf, support::Family), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-merge-files-{label}"))?;
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo)?;
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t"][..],
        &["config", "user.name", "t"][..],
    ] {
        git(&repo, args)?;
    }
    std::fs::write(repo.join("README.md"), "base\n")?;
    std::fs::write(repo.join("my notes.txt"), "old\n")?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let mut parent = repo.clone();
    if in_lane {
        parent = root.join("parent-lane");
        let at = parent.to_str().ok_or("a path is not utf-8")?;
        git(&repo, &["worktree", "add", "-q", "-b", "parent-lane", at])?;
        std::fs::write(parent.join("parent.txt"), "parent work\n")?;
        git(&parent, &["add", "parent.txt"])?;
        git(&parent, &["commit", "-qm", "parent work"])?;
    }
    std::fs::create_dir_all(root.join("home"))?;
    let store = support::memory_store(label);
    let family = support::family(root.to_path_buf(), parent, store, Some(hold));
    let mut asked = serde_json::Map::new();
    asked.insert("name".to_owned(), "writer".into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("isolation".to_owned(), "worktree".into());
    family.host.spawn("write".to_owned(), asked)?;
    assert!(
        family.reaches("writer", "finished").await,
        "{:?}",
        family.state_of("writer")
    );
    Ok((root, repo, family))
}

/// The reply's `files` as sorted `(path, added, removed)`; a reply without the key is an error.
fn files_of(
    reply: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<Changed>, Box<dyn Error>> {
    let listed = reply.get("files").and_then(serde_json::Value::as_array);
    let mut files = Vec::new();
    for file in listed.ok_or_else(|| format!("no files key: {reply:?}"))? {
        let count = |key: &str| file[key].as_u64().ok_or("a count is missing");
        let path = file["path"].as_str().ok_or("a path is missing")?;
        files.push((path.to_owned(), count("added")?, count("removed")?));
    }
    files.sort();
    Ok(files)
}

/// Dies with a reply that says a child's work landed and not which files; plain `--numstat`
/// would quote the accented name as octal escapes, and a binary file's `-` counts drop its row.
#[tokio::test]
async fn a_merged_worktree_names_the_files_it_changed() -> TestResult {
    let hold = "printf 'a\\nb\\n' >> README.md; printf 'new\\n' > 'my notes.txt'; printf '1\\n2\\n3\\n' > café.txt; printf '\\000\\001' > blob.bin";
    let (_root, repo, family) = finished_child("files", hold, false).await?;
    let merged = family.host.merge_worktree("writer")?;
    assert_eq!(merged["merged"], true, "{merged:?}");
    let listed = |path: &str, added, removed| (path.to_owned(), added, removed);
    assert_eq!(
        files_of(&merged)?,
        [
            listed("README.md", 2, 0),
            listed("blob.bin", 0, 0),
            listed("café.txt", 3, 0),
            listed("my notes.txt", 1, 1)
        ]
    );
    assert!(repo.join("café.txt").is_file());
    Ok(())
}

/// Dies with `files` inserted on the published arm only: a conflict is the reply that most
/// needs to say what the child touched.
#[tokio::test]
async fn a_conflicting_merge_names_the_files_too() -> TestResult {
    let (_root, repo, family) =
        finished_child("conflict", "printf 'child\\n' > README.md", false).await?;
    std::fs::write(repo.join("README.md"), "parent\n")?;
    git(&repo, &["commit", "-qam", "parent moved"])?;
    let merged = family.host.merge_worktree("writer")?;
    assert_eq!(merged["merged"], false, "{merged:?}");
    assert_eq!(files_of(&merged)?, [("README.md".to_owned(), 1, 1)]);
    Ok(())
}

/// Dies with the list read through the 30 KB capture, whose cut splices an omitted-bytes row
/// into the middle of it: a diff past that cap lists every file, whole.
#[tokio::test]
async fn a_list_past_the_capture_cap_is_whole() -> TestResult {
    let hold = "mkdir -p a-directory-name-long-enough-to-pad-each-path; for n in $(seq 1000 1700); do echo x > a-directory-name-long-enough-to-pad-each-path/file_$n.txt; done";
    let (_root, _repo, family) = finished_child("big", hold, false).await?;
    let merged = family.host.merge_worktree("writer")?;
    let files = files_of(&merged)?;
    assert_eq!(files.len(), 701, "{merged:?}");
    let bytes: usize = files.iter().map(|(path, ..)| path.len() + 6).sum();
    assert!(bytes > yi_tools::OUTPUT_CAP, "{bytes} bytes fit the cap");
    assert!(
        files
            .iter()
            .all(|(path, added, _)| path.ends_with(".txt") && *added == 1)
    );
    Ok(())
}

/// Dies with a lane whose state file lost its base under a parent that is itself a lane: the
/// fallback reads from the parent's `HEAD`, and the trunk's would list the parent's own commit.
#[tokio::test]
async fn a_worktree_with_no_recorded_base_still_names_its_files() -> TestResult {
    let (root, _repo, family) =
        finished_child("nobase", "printf 'x\\n' >> README.md", true).await?;
    for pool in std::fs::read_dir(root.join("home/.yi/lanes"))? {
        std::fs::remove_file(pool?.path().join("0.json"))?;
    }
    let merged = family.host.merge_worktree("writer")?;
    assert_eq!(files_of(&merged)?, [("README.md".to_owned(), 1, 0)]);
    Ok(())
}

/// Dies with a child that changed nothing, whose reply must still carry `files` as an empty list.
#[tokio::test]
async fn a_child_that_changed_nothing_lists_no_files() -> TestResult {
    let (_root, _repo, family) = finished_child("nothing", "true", false).await?;
    let merged = family.host.merge_worktree("writer")?;
    assert_eq!(files_of(&merged)?, []);
    Ok(())
}
