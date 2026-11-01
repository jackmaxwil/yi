use serde_json::{Map, Value};
use yi_tools::{BashTool, Tool, ToolKind};

fn input(command: &str) -> Map<String, Value> {
    let mut input = Map::new();
    input.insert("command".to_owned(), Value::String(command.to_owned()));
    input
}

/// Incident: a hand-split verb list called each of these a read, so the advisor and the
/// parallel scheduler treated a delete as harmless; the permission classifier sees the write.
#[test]
fn a_command_that_writes_behind_a_read_verb_is_not_a_read() {
    let bash = BashTool::default();
    for command in [
        "find . -delete",
        "fd -x rm",
        "git branch -D x",
        "env rm -rf x",
        "ls & rm -rf x",
        "find . -delete>/dev/null",
        "find . -name '*.tmp' -delete>/dev/null",
        "git branch -Dx>/dev/null",
        "echo hi>/etc/hosts",
        "rg x src>/etc/hosts",
        "rg -l Tool crates>/tmp/x",
        "cat <(rm${IFS}-rf${IFS}src)",
    ] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Exec, "{command:?}");
        assert!(bash.irreversible(&input), "{command:?}");
    }
}

/// Two searches are reads the loop runs side by side, so the classifier must keep calling them
/// reads, and so is a command that prints one named variable.
#[test]
fn read_only_searches_stay_reads() {
    let bash = BashTool::default();
    for command in [
        "rg -n parse crates",
        "rg -l Tool crates/tools 2>/dev/null",
        "ag -n parse crates",
        "printenv PATH",
    ] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Read, "{command:?}");
        assert!(!bash.irreversible(&input), "{command:?}");
    }
    // A bare `env` prints every key in the process, so a pipeline over it is not a proven read.
    for command in ["env | grep PATH", "printenv OPENAI_API_KEY"] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Exec, "{command:?}");
        assert!(bash.irreversible(&input), "{command:?}");
    }
}

mod fresh_tree {
    use crate::scratch::Scratch;
    use std::error::Error;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use yi_tools::ToolContext;
    use yi_tools::jobs::{Run, run_or_background};

    type TestResult = Result<(), Box<dyn Error>>;

    fn run(
        command: &str,
        cwd: &std::path::Path,
    ) -> Result<yi_tools::CommandCapture, Box<dyn Error>> {
        let context = ToolContext::new(cwd.to_path_buf());
        match run_or_background(command, &context, None, Duration::from_secs(30), None, None)? {
            Run::Finished(capture) => Ok(*capture),
            _ => Err("the command did not finish".into()),
        }
    }

    /// Dies with the hook off the lock: a parallel first command would run before the carry ended,
    /// or carry twice. Sixteen wait, one runs it, and each sees what it left.
    #[test]
    fn sixteen_first_commands_run_the_hook_once_and_each_sees_it_done() -> TestResult {
        let tree = Scratch::new("yi-first-use")?;
        let ran = Arc::new(AtomicUsize::new(0));
        let (counter, marker) = (Arc::clone(&ran), tree.join("carried"));
        yi_tools::register_first_use(
            &tree,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
                let wrote = std::fs::write(&marker, "x");
                vec![format!("carried: {}", wrote.is_ok())]
            }),
        );
        let results: Vec<Result<yi_tools::CommandCapture, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|n| {
                    let tree = &tree;
                    scope.spawn(move || {
                        run(&format!("touch out-{n} && cat carried"), tree)
                            .map_err(|e| e.to_string())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| Err("panicked".to_owned())))
                .collect()
        });
        let mut rows = 0;
        for result in results {
            let capture = result?;
            assert_eq!(capture.exit_code, Some(0), "{capture:?}");
            rows += capture
                .rows
                .iter()
                .filter(|row| row.contains("carried: true"))
                .count();
        }
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(rows, 1, "one command reports the work, none repeats it");
        yi_tools::forget_first_use(&tree);
        Ok(())
    }

    #[test]
    fn a_read_only_command_leaves_the_hook_for_the_first_writer() -> TestResult {
        let tree = Scratch::new("yi-first-read")?;
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        yi_tools::register_first_use(
            &tree,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }),
        );
        run("ls", &tree)?;
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        run("touch x", &tree)?;
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// Dies with a shared target dir in a fork, where a sibling's binary reads as Fresh and a test
    /// runs the wrong tree's code. Elsewhere the variables pass through.
    #[test]
    fn a_fresh_tree_builds_in_its_own_target() -> TestResult {
        let (tree, other) = (
            Scratch::new("yi-first-env")?,
            Scratch::new("yi-first-other")?,
        );
        // nextest gives each test its own process.
        unsafe { std::env::set_var("CARGO_TARGET_DIR", "/shared/target") };
        yi_tools::register_first_use(&tree, Box::new(Vec::new));
        let show = "printf '%s %s' \"${CARGO_TARGET_DIR:-unset}\" \"${CARGO_INCREMENTAL:-unset}\"";
        assert_eq!(run(show, &tree)?.stdout, "unset 0");
        assert_eq!(run(show, &other)?.stdout, "/shared/target unset");
        Ok(())
    }

    #[test]
    fn stopping_a_tree_stops_its_jobs_and_no_others() -> TestResult {
        let (tree, other) = (
            Scratch::new("yi-kill-under")?,
            Scratch::new("yi-kill-other")?,
        );
        let never: yi_tools::CancelFlag = Arc::new(|| false);
        let (inside, outside) = (
            yi_tools::jobs::spawn_job("sleep 300", &tree, &never, None, None)?,
            yi_tools::jobs::spawn_job("sleep 301", &other, &never, None, None)?,
        );
        let jobs = yi_tools::jobs::registry();
        assert_eq!(jobs.kill_under(&tree), vec!["sleep 300".to_owned()]);
        jobs.wait_settled(Some(inside), Duration::from_secs(10));
        assert!(jobs.report(inside).is_some_and(|report| report.finished));
        assert!(jobs.report(outside).is_some_and(|report| !report.finished));
        jobs.kill(outside)?;
        assert_eq!(
            jobs.kill_under(&tree),
            Vec::<String>::new(),
            "nothing left to stop"
        );
        Ok(())
    }
}

/// Dies with the sweep's old `.output()`: a docker engine that stopped answering held a check past
/// the deadline it had just hit, so the goal or plan verifier hung where it should have answered.
#[test]
fn a_hung_sweep_ends_at_its_own_bound() {
    let begun = std::time::Instant::now();
    yi_tools::jobs::run_sweep("sleep 30");
    assert!(begun.elapsed() < std::time::Duration::from_secs(20));
}
