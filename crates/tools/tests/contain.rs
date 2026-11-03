//! What Linux's sandbox grants and what it verifies. The grant lists and the seal are plain path
//! logic and run everywhere; the module at the end needs the kernel, and only the forge has one.
use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_tools::{Sandbox, reseal, verify_seal, writes_sealed};

type TestResult = Result<(), Box<dyn Error>>;

fn plain(writable: Vec<PathBuf>) -> Sandbox {
    Sandbox {
        writable,
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
    }
}

#[test]
fn a_store_inside_a_root_is_left_out_by_granting_what_sits_beside_it() -> TestResult {
    let root = Scratch::new("yi-contain-grants")?;
    let (keep, deep, store) = (root.join("keep"), root.join("a/b"), root.join("a/b/secret"));
    std::fs::create_dir_all(&keep)?;
    std::fs::create_dir_all(&store)?;
    std::fs::write(deep.join("beside"), "x")?;
    let mut sandbox = plain(vec![root.to_path_buf()]);
    sandbox.deny_read = vec![store.clone()];
    for grants in [sandbox.read_grants(None), sandbox.write_grants(None)] {
        assert!(grants.contains(&keep), "{grants:?}");
        assert!(grants.contains(&deep.join("beside")), "{grants:?}");
        for covering in [
            root.to_path_buf(),
            root.join("a"),
            deep.clone(),
            store.clone(),
        ] {
            assert!(
                !grants.contains(&covering),
                "{covering:?} reaches the store: {grants:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn a_link_to_a_store_is_no_way_around_it() -> TestResult {
    let root = Scratch::new("yi-contain-link")?;
    let (inside, store) = (root.join("tree"), root.join("store"));
    std::fs::create_dir_all(inside.join("kept"))?;
    std::fs::create_dir_all(store.join("deep"))?;
    std::os::unix::fs::symlink(&store, inside.join("to-store"))?;
    std::fs::write(inside.join("store-holder"), "x")?;
    let mut sandbox = plain(vec![inside.clone()]);
    sandbox.deny_read = vec![store.clone(), inside.join("store-holder")];
    let grants = sandbox.read_grants(None);
    assert!(grants.contains(&inside.join("kept")), "{grants:?}");
    let store_real = store.canonicalize()?;
    let reached: Vec<&PathBuf> = (grants.iter())
        .filter(|grant| {
            grant
                .canonicalize()
                .is_ok_and(|real| real.starts_with(&store_real))
        })
        .collect();
    assert_eq!(
        reached,
        Vec::<&PathBuf>::new(),
        "a grant resolves into the store"
    );
    Ok(())
}

#[test]
fn a_branch_keeps_its_directory_only_under_yi_and_other_absent_leaves_get_nothing() -> TestResult {
    let root = Scratch::new("yi-contain-leaves")?;
    let refs = root.join("common/refs/heads");
    std::fs::create_dir_all(refs.join("yi"))?;
    let sandbox = plain(vec![
        refs.join("yi/lane"),
        refs.join("yi/lane.lock"),
        refs.join("feature"),
        root.join("common/packed-refs.lock"),
    ]);
    let grants = sandbox.write_grants(None);
    assert!(grants.contains(&refs.join("yi")), "{grants:?}");
    assert!(!grants.contains(&refs), "{grants:?}");
    assert!(!grants.contains(&root.join("common")), "{grants:?}");
    assert!(grants.contains(&PathBuf::from("/dev/null")), "{grants:?}");
    Ok(())
}

#[test]
fn a_kernel_reads_and_writes_its_own_connection_dir() -> TestResult {
    let root = Scratch::new("yi-contain-own")?;
    let own = root.join("connection");
    std::fs::create_dir_all(&own)?;
    let sandbox = plain(Vec::new());
    assert!(sandbox.read_grants(Some(&own)).contains(&own));
    assert!(sandbox.write_grants(Some(&own)).contains(&own));
    assert!(!sandbox.write_grants(None).contains(&own));
    Ok(())
}

fn sealed(paths: &[PathBuf], listed: &[PathBuf]) {
    let mut sandbox = plain(Vec::new());
    sandbox.deny_write = paths.to_vec();
    sandbox.sealed = (listed.iter())
        .map(|denied| yi_tools::sandbox::Sealed {
            denied: denied.clone(),
            open: Vec::new(),
        })
        .collect();
    sandbox.seal_git_files();
}

#[test]
fn a_planted_config_is_put_back_and_refuses_one_git() -> TestResult {
    let dir = Scratch::new("yi-seal-config")?;
    let config = dir.join("config");
    std::fs::write(&config, "[core]\n")?;
    sealed(std::slice::from_ref(&config), &[]);
    assert_eq!(verify_seal(), Ok(()));
    std::fs::write(&config, "[core]\n\tfsmonitor = ./evil\n")?;
    let refused = yi_tools::host_command("git", &dir)
        .err()
        .ok_or("a tampered config let git run")?;
    assert!(
        refused
            .to_string()
            .contains("restored, and this git did not run"),
        "{refused}"
    );
    assert_eq!(std::fs::read_to_string(&config)?, "[core]\n");
    assert!(
        yi_tools::host_command("git", &dir).is_ok(),
        "restored, so the next one runs"
    );
    Ok(())
}

#[test]
fn a_new_hook_a_changed_mode_and_a_planted_pointer_all_go_back() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("yi-seal-hooks")?;
    let (hooks, pointer) = (dir.join("hooks"), dir.join("pointer"));
    std::fs::create_dir_all(&hooks)?;
    std::fs::write(hooks.join("kept.sample"), "x")?;
    sealed(&[hooks.clone(), pointer.clone()], &[]);
    std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\n")?;
    assert!(verify_seal().is_err());
    assert!(!hooks.join("pre-commit").exists());
    assert!(hooks.join("kept.sample").is_file());
    let mode = std::fs::metadata(hooks.join("kept.sample"))?
        .permissions()
        .mode();
    std::fs::set_permissions(
        hooks.join("kept.sample"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    assert!(verify_seal().is_err());
    assert_eq!(
        std::fs::metadata(hooks.join("kept.sample"))?
            .permissions()
            .mode(),
        mode
    );
    std::fs::write(&pointer, "gitdir: /elsewhere\n")?;
    assert!(verify_seal().is_err());
    assert!(!pointer.exists(), "it was absent when sealed");
    Ok(())
}

#[test]
fn an_admin_dir_a_process_made_goes_and_one_the_host_made_stays() -> TestResult {
    let dir = Scratch::new("yi-seal-names")?;
    let worktrees = dir.join("worktrees");
    std::fs::create_dir_all(worktrees.join("mine"))?;
    sealed(&[], std::slice::from_ref(&worktrees));
    std::fs::write(worktrees.join("mine/index"), "grows")?;
    assert_eq!(
        verify_seal(),
        Ok(()),
        "a listing does not read what is under a name"
    );
    std::fs::create_dir_all(worktrees.join("evil"))?;
    assert!(verify_seal().is_err());
    assert!(!worktrees.join("evil").exists());
    assert!(worktrees.join("mine").is_dir());
    std::fs::create_dir_all(worktrees.join("host-made"))?;
    reseal();
    assert_eq!(verify_seal(), Ok(()));
    assert!(worktrees.join("host-made").is_dir());
    Ok(())
}

/// Dies with a restore through the path: a link at the git dir would write the saved config elsewhere.
#[test]
fn a_git_dir_swapped_for_a_link_refuses_without_writing_through_it() -> TestResult {
    let dir = Scratch::new("yi-seal-moved")?;
    let (git, elsewhere) = (dir.join(".git"), dir.join("elsewhere"));
    std::fs::create_dir_all(&git)?;
    std::fs::create_dir_all(&elsewhere)?;
    std::fs::write(git.join("config"), "saved\n")?;
    std::fs::write(elsewhere.join("config"), "theirs\n")?;
    sealed(&[git.join("config")], &[]);
    std::fs::rename(&git, dir.join("away"))?;
    std::os::unix::fs::symlink(&elsewhere, &git)?;
    let refused = verify_seal().err().ok_or("a moved git dir passed")?;
    assert!(refused.to_string().contains("moved since"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(elsewhere.join("config"))?,
        "theirs\n"
    );
    Ok(())
}

#[test]
fn a_retired_tree_leaves_the_seal() -> TestResult {
    let dir = Scratch::new("yi-seal-retired")?;
    let tree = dir.join("tree");
    std::fs::create_dir_all(&tree)?;
    std::fs::write(tree.join(".git"), "gitdir: x\n")?;
    sealed(&[tree.join(".git")], &[]);
    std::fs::rename(&tree, dir.join("tree.gone"))?;
    assert_eq!(verify_seal(), Ok(()));
    Ok(())
}

#[test]
fn the_host_blesses_only_what_its_own_writers_wrote() -> TestResult {
    let dir = Scratch::new("yi-seal-reseal")?;
    let config = dir.join("config");
    std::fs::write(&config, "old\n")?;
    sealed(std::slice::from_ref(&config), &[]);
    std::fs::write(&config, "the host's\n")?;
    reseal();
    assert_eq!(verify_seal(), Ok(()));
    assert_eq!(std::fs::read_to_string(&config)?, "the host's\n");
    assert!(writes_sealed(&["config", "x", "y"]));
    assert!(writes_sealed(&["-c", "a=b", "worktree", "add"]));
    assert!(!writes_sealed(&["-c", "config=b", "status"]));
    assert!(writes_sealed(&["branch", "-D", "x"]));
    assert!(!writes_sealed(&["status"]));
    assert!(!writes_sealed(&["rev-parse", "HEAD"]));
    Ok(())
}

fn with_len(path: &Path, len: u64) -> TestResult {
    let file = std::fs::File::create(path)?;
    Ok(file.set_len(len)?)
}

#[test]
fn a_root_at_its_byte_limit_is_saved_and_one_past_refuses_by_name() -> TestResult {
    let limit = 16u64 << 20;
    let dir = Scratch::new("yi-seal-bytes")?;
    let fits = dir.join("fits");
    with_len(&fits, limit)?;
    sealed(std::slice::from_ref(&fits), &[]);
    assert_eq!(verify_seal(), Ok(()));
    let over = dir.join("over");
    with_len(&over, limit + 1)?;
    sealed(std::slice::from_ref(&over), &[]);
    let refused = verify_seal().err().ok_or("past the limit passed")?;
    assert!(
        refused.to_string().contains("seal.root_bytes 16777216"),
        "{refused}"
    );
    Ok(())
}

#[test]
fn a_directory_at_its_file_limit_is_saved_and_one_past_refuses_by_name() -> TestResult {
    let dir = Scratch::new("yi-seal-files")?;
    let hooks = dir.join("hooks");
    std::fs::create_dir_all(&hooks)?;
    for n in 0..1_023 {
        std::fs::write(hooks.join(format!("h{n}")), "")?;
    }
    sealed(std::slice::from_ref(&hooks), &[]);
    assert_eq!(verify_seal(), Ok(()));
    let more = dir.join("more");
    std::fs::create_dir_all(&more)?;
    for n in 0..1_024 {
        std::fs::write(more.join(format!("h{n}")), "")?;
    }
    sealed(std::slice::from_ref(&more), &[]);
    let refused = verify_seal().err().ok_or("past the limit passed")?;
    assert!(
        refused.to_string().contains("seal.root_files 1024"),
        "{refused}"
    );
    Ok(())
}

#[test]
fn the_seal_counts_its_roots_to_its_limit_and_refuses_one_past() -> TestResult {
    let dir = Scratch::new("yi-seal-roots")?;
    let at = |range: std::ops::Range<usize>| -> Vec<PathBuf> {
        range.map(|n| dir.join(format!("absent{n}"))).collect()
    };
    sealed(&at(0..8_192), &[]);
    assert_eq!(verify_seal(), Ok(()));
    sealed(&at(8_192..8_193), &[]);
    let refused = verify_seal().err().ok_or("past the limit passed")?;
    assert!(refused.to_string().contains("seal.roots 8192"), "{refused}");
    Ok(())
}

/// Only here can a test watch the kernel refuse: a forge run sets `YI_SANDBOX_REQUIRED`, so a kernel
/// with no Landlock fails the suite where a Mac or Docker Desktop returns early.
#[cfg(target_os = "linux")]
mod kernel {
    use super::*;

    fn contained() -> Result<bool, Box<dyn Error>> {
        let exe = std::env::current_exe()?;
        let yi = exe.ancestors().nth(2).ok_or("no target dir")?.join("yi");
        yi_tools::set_wrapper(yi);
        let required = std::env::var_os("YI_SANDBOX_REQUIRED").is_some();
        match (Sandbox::available(), required) {
            (true, _) => Ok(true),
            (false, true) => Err("YI_SANDBOX_REQUIRED, and Landlock did not apply".into()),
            (false, false) => Ok(false),
        }
    }

    fn sh(sandbox: &Sandbox, script: &str) -> Result<std::process::Output, Box<dyn Error>> {
        let (program, args) = sandbox.wrap("/bin/sh", &["-c", script]);
        Ok(yi_tools::keyless_command(program).args(args).output()?)
    }

    #[test]
    fn a_write_outside_the_roots_and_a_read_of_a_store_are_refused() -> TestResult {
        if !contained()? {
            return Ok(());
        }
        let root = Scratch::new("yi-landlock")?;
        let (tree, outside, store) = (
            root.join("tree"),
            root.join("outside"),
            root.join("tree/.ssh"),
        );
        std::fs::create_dir_all(&outside)?;
        std::fs::create_dir_all(&store)?;
        std::fs::create_dir_all(tree.join("sub"))?;
        std::fs::write(store.join("id"), "PRIVATE\n")?;
        let mut sandbox = plain(vec![tree.clone()]);
        sandbox.deny_read = vec![store.clone()];
        let write = sh(
            &sandbox,
            &format!("echo x > '{}'/marker", outside.display()),
        )?;
        assert!(!write.status.success());
        assert!(!outside.join("marker").exists());
        let read = sh(&sandbox, &format!("cat '{}'/id", store.display()))?;
        assert!(
            !read.status.success(),
            "{}",
            String::from_utf8_lossy(&read.stdout)
        );
        let inside = sh(&sandbox, &format!("echo x > '{}'/sub/f", tree.display()))?;
        assert!(inside.status.success(), "{inside:?}");
        assert!(tree.join("sub/f").is_file());
        Ok(())
    }

    /// `/proc` is readable whole because a rule cannot name a pid its grandchildren will have.
    /// Dies with Landlock's ptrace scope gone: the host's environment would be one `cat` away.
    #[test]
    fn a_contained_process_cannot_read_another_process_environment() -> TestResult {
        if !contained()? {
            return Ok(());
        }
        let root = Scratch::new("yi-landlock-proc")?;
        let sandbox = plain(vec![root.to_path_buf()]);
        let ours = std::process::id();
        let read = sh(&sandbox, &format!("cat /proc/{ours}/environ"))?;
        assert!(
            !read.status.success(),
            "{}",
            String::from_utf8_lossy(&read.stdout)
        );
        assert!(read.stdout.is_empty());
        let own = sh(&sandbox, "cat /proc/self/status")?;
        assert!(own.status.success());
        Ok(())
    }

    #[test]
    fn a_contained_git_config_runs_nothing_on_the_host_and_goes_back() -> TestResult {
        if !contained()? {
            return Ok(());
        }
        let root = Scratch::new("yi-landlock-git")?;
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo)?;
        let run = |args: &[&str]| {
            yi_tools::keyless_command("git")
                .args(args)
                .current_dir(&repo)
                .status()
        };
        assert!(run(&["init", "-q", "-b", "main"])?.success());
        let before = std::fs::read(repo.join(".git/config"))?;
        let sandbox = Sandbox::for_workspace(&repo, &root, None);
        let script = format!(
            "cd '{}' && git config core.sshCommand ./evil",
            repo.display()
        );
        let planted = sh(&sandbox, &script)?;
        assert!(
            planted.status.success(),
            "a writable root cannot deny a file inside it: {planted:?}"
        );
        let refused = yi_tools::host_command("git", &repo)
            .err()
            .ok_or("host git ran over a planted config")?;
        assert!(refused.to_string().contains("restored"), "{refused}");
        assert_eq!(std::fs::read(repo.join(".git/config"))?, before);
        Ok(())
    }
}
