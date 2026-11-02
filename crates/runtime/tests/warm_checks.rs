//! Checks on warm directories. The reference a verdict answers to is the same command in a clean
//! worktree of the same commit: one wrong verdict kills sharing, and `rm -rf target` per check returns.
use crate::git_lane::git;
use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use yi_runtime::lane::warm::{
    Asked, CHECK_QUEUE, Verdict, WarmRun, check_commit, scrub, snapshot, ticket,
};
use yi_runtime::lane::{ClaimBase, Pool, Sha};

type TestResult = Result<(), Box<dyn Error>>;

struct Rig {
    _root: Scratch,
    repo: PathBuf,
    warm: Pool,
    checker: yi_runtime::goal::Checker,
}

fn rig(tag: &str, slots: u8) -> Result<Rig, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-warm-{tag}"))?;
    let (repo, home) = (root.join("repo"), root.join("home"));
    std::fs::create_dir_all(&repo)?;
    std::fs::create_dir_all(home.join(".yi"))?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join(".gitignore"), "out/\ntarget/\n.venv/\n")?;
    std::fs::write(repo.join("a.txt"), "a\n")?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "first"])?;
    let mut card: serde_json::Value = serde_json::to_value(yi_runtime::node::card(&home)?)?;
    card["slots"] = slots.into();
    std::fs::write(home.join(".yi/node.json"), serde_json::to_vec(&card)?)?;
    let warm = Pool::open(&home, &repo, 1)?.warm()?;
    Ok(Rig {
        _root: root,
        repo,
        warm,
        checker: yi_runtime::goal::Checker::default(),
    })
}

impl Rig {
    fn head(&self) -> Result<Sha, Box<dyn Error>> {
        let head = git(&self.repo, &["rev-parse", "HEAD"])?;
        Ok(Sha::parse(&head).ok_or("no sha")?)
    }

    fn check(&self, command: &str) -> Result<WarmRun, Box<dyn Error>> {
        let until = Instant::now()
            .checked_add(CHECK_QUEUE)
            .ok_or("no instant")?;
        let budget = Duration::from_secs(40);
        Ok(check_commit(
            &self.warm,
            &self.head()?,
            &self.repo,
            command,
            until,
            budget,
            &self.checker,
        ))
    }
}

#[test]
fn an_ignored_report_from_one_borrower_is_gone_for_the_next() -> TestResult {
    let rig = rig("report", 1)?;
    let first = rig.check("mkdir -p out && echo mine > out/report && test -s out/report")?;
    assert_eq!(first.verdict, Verdict::Green, "{first:?}");
    let next = rig.check("test ! -e out/report")?;
    assert_eq!(next.verdict, Verdict::Green, "{next:?}");
    Ok(())
}

/// Dies with `target/<profile>` kept whole: a test that runs `./target/debug/app` with no build would
/// run the last borrower's binary, and pass.
#[test]
fn a_stale_binary_is_gone_and_cargos_bookkeeping_stays() -> TestResult {
    let rig = rig("stale", 1)?;
    let build = "mkdir -p target/debug/deps .venv && echo unit > target/debug/deps/unit && echo env > .venv/pin && echo bin > target/debug/app";
    assert_eq!(rig.check(build)?.verdict, Verdict::Green);
    let next = rig.check(
        "test ! -e target/debug/app && test -s target/debug/deps/unit && test ! -e .venv/pin",
    )?;
    assert_eq!(next.verdict, Verdict::Green, "{next:?}");
    Ok(())
}

/// Dies with the release leaving the tree as the check left it: the pool reads a dirty slot as an
/// orphan, so a one-slot pool waits out the whole queue. The fixture is what `cargo test` does with no
/// committed lockfile, and what a test that edits a tracked file does.
#[test]
fn a_check_that_writes_and_edits_still_frees_its_directory() -> TestResult {
    let rig = rig("frees", 1)?;
    assert_eq!(
        rig.check("echo x > Cargo.lock; echo y >> a.txt")?.verdict,
        Verdict::Green
    );
    let until = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("no instant")?;
    let next = check_commit(
        &rig.warm,
        &rig.head()?,
        &rig.repo,
        "test ! -e Cargo.lock && test \"$(cat a.txt)\" = a",
        until,
        Duration::from_secs(5),
        &rig.checker,
    );
    assert_eq!(next.verdict, Verdict::Green, "{next:?}");
    Ok(())
}

/// Dies with the slot left as the failed release found it: a one-slot pool then answers every later
/// check with "no warm directory came free", though nothing holds it.
#[test]
fn a_release_that_cannot_clean_still_frees_its_directory() -> TestResult {
    let rig = rig("release-fails", 1)?;
    let head = rig.head()?;
    let held = rig
        .warm
        .claim("first", ClaimBase::Commit(head.as_str().to_owned()))?;
    std::fs::write(held.path().join(".git"), "gitdir: /nowhere\n")?;
    assert!(held.release_warm().is_err(), "a broken tree cleaned");
    let until = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("no instant")?;
    let next = check_commit(
        &rig.warm,
        &head,
        &rig.repo,
        "test -s a.txt",
        until,
        Duration::from_secs(5),
        &rig.checker,
    );
    assert_eq!(next.verdict, Verdict::Green, "{next:?}");
    Ok(())
}

#[test]
fn a_red_check_says_its_exit_and_the_output_names_the_model_tree() -> TestResult {
    let rig = rig("red", 1)?;
    let run = rig.check("pwd; exit 3")?;
    assert_eq!(run.verdict, Verdict::Red { exit: Some(3) });
    let shown = rig.repo.to_string_lossy().into_owned();
    let output = run.output();
    assert!(output.contains(&shown), "{output}");
    assert!(!output.contains("/warm/"), "{output}");
    Ok(())
}

#[test]
fn past_its_budget_a_check_is_no_verdict_on_the_work() -> TestResult {
    let rig = rig("budget", 1)?;
    let until = Instant::now()
        .checked_add(CHECK_QUEUE)
        .ok_or("no instant")?;
    let run = check_commit(
        &rig.warm,
        &rig.head()?,
        &rig.repo,
        "sleep 30",
        until,
        Duration::from_millis(200),
        &rig.checker,
    );
    let Verdict::Abstain { why } = run.verdict else {
        return Err(format!("a cut check read as {:?}", run.verdict).into());
    };
    assert!(why.contains("budget"), "{why}");
    Ok(())
}

#[test]
fn past_its_queue_a_check_names_the_pool_that_held_it() -> TestResult {
    let rig = rig("queue", 1)?;
    let head = rig.head()?;
    let held = rig.warm.claim(
        "holder",
        yi_runtime::lane::ClaimBase::Commit(head.as_str().to_owned()),
    )?;
    let until = Instant::now()
        .checked_add(Duration::from_millis(100))
        .ok_or("no instant")?;
    let run = check_commit(
        &rig.warm,
        &head,
        &rig.repo,
        "true",
        until,
        Duration::from_secs(5),
        &rig.checker,
    );
    let Verdict::Abstain { why } = run.verdict else {
        return Err(format!("a queued check read as {:?}", run.verdict).into());
    };
    assert!(why.contains("1 of 1 held (node.slots 1)"), "{why}");
    held.release_warm()?;
    Ok(())
}

#[test]
fn three_checks_through_two_directories_all_answer_and_the_wait_is_counted() -> TestResult {
    let rig = rig("through", 2)?;
    let runs: Vec<Result<WarmRun, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let rig = &rig;
                scope.spawn(move || rig.check("sleep 1").map_err(|error| error.to_string()))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| Err("panicked".to_owned())))
            .collect()
    });
    let mut waited = Duration::ZERO;
    for run in runs {
        let run = run?;
        assert_eq!(run.verdict, Verdict::Green, "{run:?}");
        waited = waited.max(run.waited);
    }
    assert!(
        waited >= Duration::from_millis(500),
        "the third never waited: {waited:?}"
    );
    Ok(())
}

#[test]
fn a_snapshot_holds_dirt_and_new_files_and_moves_nothing_the_child_has() -> TestResult {
    let rig = rig("snapshot", 1)?;
    let tree = &rig.repo;
    std::fs::write(tree.join("a.txt"), "edited\n")?;
    std::fs::write(tree.join("new.txt"), "new\n")?;
    std::fs::create_dir_all(tree.join("out"))?;
    std::fs::write(tree.join("out/ignored"), "x")?;
    git(tree, &["add", "a.txt"])?;
    let (head, status) = (rig.head()?, git(tree, &["status", "--porcelain"])?);
    let commit = snapshot(tree)?;
    assert_eq!(rig.head()?, head);
    assert_eq!(
        git(tree, &["status", "--porcelain"])?,
        status,
        "the child's staging stands"
    );
    assert_eq!(
        git(tree, &["show", &format!("{}:a.txt", commit.as_str())])?,
        "edited"
    );
    assert_eq!(
        git(tree, &["show", &format!("{}:new.txt", commit.as_str())])?,
        "new"
    );
    let listed = git(tree, &["ls-tree", "-r", "--name-only", commit.as_str()])?;
    assert!(!listed.contains("out/ignored"), "{listed}");
    Ok(())
}

#[test]
fn a_scrub_leaves_the_rebuild_state_and_nothing_else_untracked() -> TestResult {
    let rig = rig("scrub", 1)?;
    let tree = &rig.repo;
    for (dir, file) in [
        ("target/debug", "app"),
        ("target/debug/deps", "unit"),
        ("target/release/.fingerprint", "f"),
        (".venv", "pin"),
        ("out", "x"),
        ("loose", "y"),
    ] {
        std::fs::create_dir_all(tree.join(dir))?;
        std::fs::write(tree.join(dir).join(file), "x")?;
    }
    scrub(tree)?;
    for gone in ["target/debug/app", ".venv/pin", "out", "loose"] {
        assert!(!tree.join(gone).exists(), "{gone} survived");
    }
    for kept in [
        "target/debug/deps/unit",
        "target/release/.fingerprint/f",
        "a.txt",
    ] {
        assert!(tree.join(kept).exists(), "{kept} went");
    }
    Ok(())
}

fn green() -> WarmRun {
    WarmRun {
        verdict: Verdict::Green,
        capture: None,
        waited: Duration::ZERO,
    }
}

/// Dies with the run tied to the cell: a check past the kernel's 600 s abort would start over each
/// time, and at 128 children never finish.
#[test]
fn a_ticket_outlives_its_first_wait_is_read_once_and_runs_once() -> TestResult {
    let starts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = |wait: Duration, starts: &std::sync::Arc<std::sync::atomic::AtomicUsize>| {
        let starts = std::sync::Arc::clone(starts);
        ticket("tree", "cmd", wait, move || {
            starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            green()
        })
    };
    let Asked::Waiting { row } = run(Duration::ZERO, &starts) else {
        return Err("a running check answered".into());
    };
    assert!(row.contains("the same call again reads this run"), "{row}");
    let Asked::Finished(first) = run(Duration::from_secs(10), &starts) else {
        return Err("the check never finished".into());
    };
    assert_eq!(first.verdict, Verdict::Green);
    assert_eq!(
        starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second call read the same run"
    );
    let _a_third_call_is_a_new_run = run(Duration::from_secs(10), &starts);
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn the_ticket_table_holds_at_its_limit_and_refuses_one_past() -> TestResult {
    for n in 0..256 {
        let Asked::Waiting { .. } = ticket("full", &n.to_string(), Duration::ZERO, || {
            std::thread::sleep(Duration::from_secs(5));
            green()
        }) else {
            return Err(format!("ticket {n} was refused").into());
        };
    }
    let Asked::Finished(refused) = ticket("full", "one past", Duration::ZERO, green) else {
        return Err("one past the table ran".into());
    };
    let Verdict::Abstain { why } = refused.verdict else {
        return Err("one past the table has a verdict".into());
    };
    assert!(why.contains("check.tickets 256"), "{why}");
    yi_runtime::lane::warm::forget_tickets("full");
    assert!(
        matches!(ticket("full", "again", Duration::from_secs(1), green), Asked::Finished(run) if run.verdict == Verdict::Green)
    );
    Ok(())
}

fn write(path: &Path, text: &str) -> TestResult {
    if let Some(up) = path.parent() {
        std::fs::create_dir_all(up)?;
    }
    Ok(std::fs::write(path, text)?)
}

fn crate_at(repo: &Path, name: &str, test: &str) -> TestResult {
    write(
        &repo.join(name).join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )?;
    write(
        &repo.join(name).join("src/lib.rs"),
        &format!("#[test]\nfn it() {{ {test} }}\n"),
    )
}

fn commit(repo: &Path, message: &str) -> TestResult {
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "--allow-empty", "-m", message])?;
    Ok(())
}

fn clean_exit(repo: &Path, scratch: &Path, command: &str) -> Result<Option<i32>, Box<dyn Error>> {
    let tree = scratch.join("clean");
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            tree.to_string_lossy().as_ref(),
            "HEAD",
        ],
    )?;
    let out = yi_tools::keyless_command("sh")
        .args(["-c", command])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(&tree)
        .output()?;
    git(
        repo,
        &[
            "worktree",
            "remove",
            "-f",
            "-f",
            tree.to_string_lossy().as_ref(),
        ],
    )?;
    Ok(out.status.code())
}

/// The kill test for sharing, on a rotation a warm directory could get wrong: an edit, a deletion a
/// test reads, a new file, a test that reads ignored output, and back. Each verdict must equal a clean
/// build's, in the same directory one borrower after another. Dies with the scrub off.
#[test]
fn a_warm_verdict_agrees_with_a_clean_worktree_across_a_rotation() -> TestResult {
    let rig = rig("bet", 1)?;
    let (repo, scratch) = (&rig.repo, Scratch::new("yi-warm-bet-clean")?);
    // A check's environment is an allowlist, and a test's HOME holds no toolchain.
    let exports: String = ["RUSTUP_HOME", "CARGO_HOME", "RUSTUP_TOOLCHAIN"]
        .iter()
        .filter_map(|name| Some(format!("export {name}='{}'; ", std::env::var(name).ok()?)))
        .collect();
    let command = format!("{exports}cargo test --offline --quiet --workspace 2>&1");
    let command = command.as_str();
    write(
        &repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"one\", \"two\"]\nresolver = \"2\"\n",
    )?;
    write(&repo.join("data.txt"), "present\n")?;
    crate_at(repo, "one", "assert_eq!(1, 1);")?;
    crate_at(
        repo,
        "two",
        "assert!(std::path::Path::new(\"../data.txt\").exists());",
    )?;
    commit(repo, "base")?;
    let steps: [(&str, &dyn Fn() -> TestResult, bool); 5] = [
        ("base", &|| Ok(()), true),
        (
            "a broken edit",
            &|| crate_at(repo, "one", "assert_eq!(1, 2);"),
            false,
        ),
        (
            "fixed, a file a test reads deleted",
            &|| {
                crate_at(repo, "one", "assert_eq!(1, 1);")?;
                Ok(std::fs::remove_file(repo.join("data.txt"))?)
            },
            false,
        ),
        (
            "restored, and a test of ignored output a borrower left",
            &|| {
                write(&repo.join("data.txt"), "present\n")?;
                crate_at(
                    repo,
                    "one",
                    "assert!(!std::path::Path::new(\"../out/left\").exists());",
                )
            },
            true,
        ),
        (
            "a new file",
            &|| write(&repo.join("one/src/extra.rs"), "// new\n"),
            true,
        ),
    ];
    for (label, apply, green) in steps {
        apply()?;
        commit(repo, label)?;
        let clean = clean_exit(repo, &scratch, command)?;
        let warm = rig.check(&format!(
            "{command}; code=$?; mkdir -p out && echo x > out/left; exit $code"
        ))?;
        let reads_green = warm.verdict == Verdict::Green;
        assert_eq!(
            reads_green,
            clean == Some(0),
            "{label}: warm {:?} clean {clean:?}\n{}",
            warm.verdict,
            warm.output()
        );
        assert_eq!(
            reads_green,
            green,
            "{label}: the rotation's own expectation\n{}",
            warm.output()
        );
    }
    Ok(())
}
