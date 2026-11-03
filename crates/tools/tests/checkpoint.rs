use std::fs;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FOREIGN_INDEX: &str = "yi-checkpoint-foreign-index";

#[test]
fn undo_restores_and_removes_non_ascii_paths() -> TestResult {
    let root = Scratch::new("yi-checkpoint-non-ascii")?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    fs::write(project.join("naïve.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&root.join("shadow"), &project)?;
    let turn_start = checkpoints.capture()?;
    fs::write(project.join("naïve.txt"), "after\n")?;
    fs::write(project.join("日本.txt"), "new\n")?;

    let mut changed = checkpoints.restore(&turn_start, None)?;
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    let names: Vec<String> = changed
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    assert_eq!(names, ["naïve.txt", "日本.txt"]);
    assert_eq!(fs::read_to_string(project.join("naïve.txt"))?, "before\n");
    assert!(
        !project.join("日本.txt").exists(),
        "the added file is removed"
    );
    Ok(())
}

#[test]
fn a_capture_leaves_the_callers_git_index_untouched() -> TestResult {
    let foreign = std::env::var_os("GIT_INDEX_FILE")
        .filter(|path| path.to_string_lossy().ends_with(FOREIGN_INDEX));
    let Some(index) = foreign else {
        let root = Scratch::new("yi-checkpoint-index-rerun")?;
        let test = crate::exact_path(
            module_path!(),
            "a_capture_leaves_the_callers_git_index_untouched",
        );
        let rerun = yi_tools::command(std::env::current_exe()?)
            .args(["--exact", &test])
            .env("GIT_INDEX_FILE", root.join(FOREIGN_INDEX))
            .output()?;
        let said = String::from_utf8_lossy(&rerun.stdout) + String::from_utf8_lossy(&rerun.stderr);
        assert!(
            rerun.status.success() && said.contains("1 passed"),
            "{said}"
        );
        return Ok(());
    };
    let root = Scratch::new("yi-checkpoint-index")?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    fs::write(project.join("kept.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&root.join("shadow"), &project)?;
    let tree = checkpoints.capture()?;
    fs::write(project.join("kept.txt"), "after\n")?;
    checkpoints.restore(&tree, None)?;
    assert_eq!(fs::read_to_string(project.join("kept.txt"))?, "before\n");
    assert!(
        !std::path::Path::new(&index).exists(),
        "the shadow git wrote the caller's GIT_INDEX_FILE"
    );
    Ok(())
}

mod one_store {
    use super::*;
    use yi_tools::Checkpoints;

    fn git(dir: &std::path::Path, args: &[&str]) -> TestResult {
        let status = yi_tools::keyless_command("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(dir)
            .status()?;
        assert!(status.success(), "git {args:?}");
        Ok(())
    }

    fn repo(root: &std::path::Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let main = root.join("main");
        std::fs::create_dir_all(&main)?;
        git(&main, &["init", "-q", "-b", "main"])?;
        git(&main, &["commit", "-q", "--allow-empty", "-m", "first"])?;
        Ok(main)
    }

    fn stores(checkpoints: &std::path::Path) -> Result<usize, std::io::Error> {
        Ok(std::fs::read_dir(checkpoints)?.count())
    }

    /// Dies with an index per store, not per tree: a capture took a sibling's staged files.
    #[test]
    fn parallel_trees_share_one_store_and_each_captures_only_its_own() -> TestResult {
        let root = Scratch::new("yi-one-store")?;
        let main = repo(&root)?;
        let store = root.join("checkpoints");
        let mut trees = Vec::new();
        for at in 0..8 {
            let tree = root.join(format!("tree-{at}"));
            git(
                &main,
                &["worktree", "add", "-q", "--detach", &tree.to_string_lossy()],
            )?;
            std::fs::write(tree.join(format!("only-{at}.txt")), "mine\n")?;
            trees.push(tree);
        }
        let handles: Vec<_> = trees
            .iter()
            .cloned()
            .enumerate()
            .map(|(at, tree)| {
                let store = store.clone();
                std::thread::spawn(move || -> Result<(), String> {
                    let checkpoints =
                        Checkpoints::open(&store, &tree).map_err(|e| e.to_string())?;
                    let captured = checkpoints.capture().map_err(|e| e.to_string())?;
                    for other in 0..8 {
                        let shown = checkpoints.show(&captured, &format!("only-{other}.txt"));
                        assert_eq!(shown.is_ok(), other == at, "tree {at} holding only-{other}");
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "a capture panicked")??;
        }
        assert_eq!(stores(&store)?, 1, "one store per repository");
        Ok(())
    }

    /// An old session's `/undo` names a tree in the per-path store, which ages out on its own.
    #[test]
    fn a_tree_from_before_the_shared_store_is_served_from_the_old_one() -> TestResult {
        let root = Scratch::new("yi-legacy-store")?;
        let store = root.join("checkpoints");
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("old.txt"), "before\n")?;
        // A directory that is no repository keys by its path, which is the old store's name.
        let old = Checkpoints::open(&store, &project)?.capture()?;
        git(&project, &["init", "-q", "-b", "main"])?;
        std::fs::write(project.join("old.txt"), "after\n")?;
        let held = Checkpoints::open_holding(&store, &project, &old)?;
        assert_eq!(held.show(&old, "old.txt")?, "before\n");
        Ok(())
    }

    /// Retention ages a store by its directory's mtime: an index in a subdirectory would let a
    /// prune race captures.
    #[test]
    fn a_capture_moves_the_store_dirs_mtime() -> TestResult {
        let root = Scratch::new("yi-store-mtime")?;
        let main = repo(&root)?;
        let store = root.join("checkpoints");
        let checkpoints = Checkpoints::open(&store, &main)?;
        let dir = std::fs::read_dir(&store)?.next().ok_or("no store")??.path();
        let then = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::open(&dir)?.set_modified(then)?;
        checkpoints.capture()?;
        assert!(
            std::fs::metadata(&dir)?.modified()? > then,
            "the mtime stayed"
        );
        Ok(())
    }

    #[test]
    fn the_repositorys_exclude_list_reaches_the_shared_store() -> TestResult {
        let root = Scratch::new("yi-store-exclude")?;
        let main = repo(&root)?;
        let store = root.join("checkpoints");
        std::fs::write(main.join("secret.env"), "x\n")?;
        std::fs::write(main.join(".git/info/exclude"), "secret.env\n")?;
        let tree = Checkpoints::open(&store, &main)?.capture()?;
        assert!(
            Checkpoints::open(&store, &main)?
                .show(&tree, "secret.env")
                .is_err()
        );
        std::fs::write(main.join("later.env"), "x\n")?;
        std::fs::write(main.join(".git/info/exclude"), "secret.env\nlater.env\n")?;
        let checkpoints = Checkpoints::open(&store, &main)?;
        let tree = checkpoints.capture()?;
        assert!(
            checkpoints.show(&tree, "later.env").is_err(),
            "an edit reached the next open"
        );
        Ok(())
    }

    /// The prototype's `checkStat=minimal` missed this rewrite, so it is not ported.
    #[test]
    fn a_same_size_same_mtime_rewrite_is_seen() -> TestResult {
        let root = Scratch::new("yi-store-racy")?;
        let main = repo(&root)?;
        let file = main.join("data.txt");
        std::fs::write(&file, "aaaa\n")?;
        let stamp =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)?
            .set_modified(stamp)?;
        let checkpoints = Checkpoints::open(&root.join("checkpoints"), &main)?;
        let before = checkpoints.capture()?;
        // Git reads ctime in whole seconds, so a rewrite within one is out of this store's reach.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, "bbbb\n")?;
        std::fs::File::options()
            .write(true)
            .open(&file)?
            .set_modified(stamp)?;
        let after = checkpoints.capture()?;
        assert_ne!(before, after, "the rewrite was missed");
        Ok(())
    }
}

mod deadline {
    use crate::scratch::Scratch;
    use std::error::Error;
    use std::time::Duration;
    use yi_tools::{CheckpointError, Checkpoints};

    /// A git ahead of the real one on PATH, slow only for `add`. nextest gives each test its own
    /// process, so it reaches no other test.
    fn slow_add(dir: &Scratch) -> Result<(), Box<dyn Error>> {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::var_os("PATH").ok_or("no PATH")?;
        let real = (std::env::split_paths(&path).map(|dir| dir.join("git")))
            .find(|candidate| candidate.is_file())
            .ok_or("no git on PATH")?;
        let marker = dir.join("slow");
        let body = format!(
            "#!/bin/sh\nif [ -e '{}' ]; then case \"$*\" in *' add '*) touch \"$GIT_INDEX_FILE.lock\"; sleep 30;; esac; fi\nexec '{}' \"$@\"\n",
            marker.display(),
            real.display()
        );
        let shim = dir.join("git");
        std::fs::write(&shim, body)?;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
        let joined = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path)),
        )?;
        unsafe { std::env::set_var("PATH", joined) };
        Ok(())
    }

    /// Dies with the lock left behind: every capture after a kill would fail on `index.lock`.
    #[test]
    fn a_killed_capture_clears_its_lock_and_the_next_one_works() -> Result<(), Box<dyn Error>> {
        let (root, shim) = (
            Scratch::new("yi-capture-kill")?,
            Scratch::new("yi-capture-shim")?,
        );
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("a.txt"), "a\n")?;
        slow_add(&shim)?;
        let checkpoints = Checkpoints::open(&root.join("store"), &project)?;
        std::fs::write(shim.join("slow"), "")?;
        let begun = std::time::Instant::now();
        let killed = checkpoints.capture_within(Duration::from_millis(200));
        assert!(
            matches!(killed, Err(CheckpointError::Deadline { .. })),
            "{killed:?}"
        );
        assert!(
            begun.elapsed() < Duration::from_secs(20),
            "the kill did not end it"
        );
        std::fs::remove_file(shim.join("slow"))?;
        assert!(checkpoints.capture_within(Duration::from_secs(30)).is_ok());
        assert!(checkpoints.capture().is_ok());
        Ok(())
    }

    /// Dies with the clean-up taking any lock: another process's would go, and two gits write one index.
    #[test]
    fn a_killed_capture_leaves_a_lock_that_was_there_before_it() -> Result<(), Box<dyn Error>> {
        let (root, shim) = (
            Scratch::new("yi-capture-other")?,
            Scratch::new("yi-capture-shim2")?,
        );
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("a.txt"), "a\n")?;
        slow_add(&shim)?;
        let store = root.join("store");
        let checkpoints = Checkpoints::open(&store, &project)?;
        // The index exists only once a capture has written it.
        checkpoints.capture()?;
        let index = (std::fs::read_dir(&store)?.flatten())
            .flat_map(|repo| {
                std::fs::read_dir(repo.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("HEAD"))
            });
        let repo = index
            .and_then(|head| head.parent().map(std::path::Path::to_path_buf))
            .ok_or("no store")?;
        let theirs = repo.join(format!("{}.lock", index_name(&repo, &project)?));
        std::fs::write(&theirs, "")?;
        std::fs::write(shim.join("slow"), "")?;
        let killed = checkpoints.capture_within(Duration::from_millis(200));
        assert!(killed.is_err(), "{killed:?}");
        assert!(theirs.exists(), "another process's lock went");
        Ok(())
    }

    fn index_name(
        repo: &std::path::Path,
        _project: &std::path::Path,
    ) -> Result<String, Box<dyn Error>> {
        let found = (std::fs::read_dir(repo)?.flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with("index-"));
        Ok(found.ok_or("no index in the store")?)
    }

    #[test]
    fn within_its_deadline_a_capture_is_the_plain_one() -> Result<(), Box<dyn Error>> {
        let root = Scratch::new("yi-capture-within")?;
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("a.txt"), "a\n")?;
        let checkpoints = Checkpoints::open(&root.join("store"), &project)?;
        let bound = checkpoints.capture_within(Duration::from_secs(30))?;
        assert_eq!(bound, checkpoints.capture()?);
        Ok(())
    }
}
