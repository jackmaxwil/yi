#![cfg(target_os = "macos")]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_tools::{CancelFlag, Run, Sandbox, denial_hint, run_or_background};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn workspace(tag: &str) -> Result<(Scratch, PathBuf, PathBuf), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-sandbox-{tag}"))?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::write(home.join(".ssh/id_rsa"), "PRIVATE KEY")?;
    std::fs::write(home.join("notes.md"), "ordinary")?;
    Ok((root, project, home))
}

fn run(command: &str, cwd: &Path, sandbox: Option<&Sandbox>) -> Result<(i32, String), String> {
    let cancelled: CancelFlag = Arc::new(|| false);
    let timeout = std::time::Duration::from_secs(120);
    match run_or_background(command, cwd, &cancelled, None, timeout, sandbox)? {
        Run::Finished(capture) => Ok((
            capture.exit_code.unwrap_or(-1),
            format!("{}{}", capture.stdout, capture.stderr),
        )),
        Run::Backgrounded(_) => Err("nothing here backgrounds".to_owned()),
        Run::TimedOut(_) => Err("nothing here runs two minutes".to_owned()),
    }
}

#[test]
fn the_policy_denies_by_default_and_names_its_roots() -> TestResult {
    let sandbox = Sandbox::for_workspace(
        Path::new("/repo"),
        Path::new("/home/user"),
        Some(Path::new("/home/user/.yi/sessions")),
    );
    let policy = sandbox.policy();
    assert!(
        policy.contains("(deny default)"),
        "the base policy is first"
    );
    assert!(policy.contains("(allow file-read*"));
    assert!(policy.contains("(allow file-write*"));
    assert!(
        policy.contains("(deny file-write-unlink"),
        "a contained process must not replace its own boundary"
    );
    assert!(
        !policy.contains("network-outbound") && !policy.contains("(allow network"),
        "no network rule at all is how egress stays off: {policy}"
    );

    let params = sandbox.params();
    let keys: Vec<&str> = params.iter().map(|(key, _)| key.as_str()).collect();
    assert!(keys.contains(&"WRITABLE_ROOT_0"));
    assert!(keys.contains(&"DENY_READ_0"));
    assert!(
        params.iter().any(|(_, path)| path == Path::new("/repo")),
        "the working tree is writable"
    );
    assert!(
        params
            .iter()
            .any(|(_, path)| path == Path::new("/home/user/.ssh")),
        "credential stores are read-denied"
    );

    let (program, args) = sandbox.wrap("sh", &["-c", "ls"]);
    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert_eq!(args.first().map(String::as_str), Some("-p"));
    let tail: Vec<&str> = args.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(tail, vec!["ls", "-c", "sh"], "the command comes last");
    assert!(args.iter().any(|arg| arg == "--"));
    assert!(args.iter().any(|arg| arg.starts_with("-DWRITABLE_ROOT_0=")));

    let kernel = sandbox.kernel_policy();
    assert!(
        kernel.contains("network-bind") && kernel.contains("localhost"),
        "the kernel profile needs loopback ZMQ"
    );
    assert!(
        !sandbox.policy().contains("network-outbound")
            && !sandbox.policy().contains("(allow network"),
        "bash wrap stays egress-off"
    );
    assert!(
        !kernel.contains("network-outbound") && !kernel.contains("system-socket"),
        "the kernel binds and accepts; it never connects out, not even to localhost"
    );
    let (program, prefix) = sandbox.kernel_prefix();
    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert_eq!(prefix.last().map(String::as_str), Some("--"));
    Ok(())
}

#[test]
fn a_denial_is_only_claimed_when_the_output_says_so() {
    assert!(denial_hint(Some(0), "operation not permitted").is_none());
    assert!(denial_hint(Some(127), "command not found").is_none());
    assert!(denial_hint(Some(1), "assertion failed").is_none());
    let hint = denial_hint(
        Some(1),
        "sh: cannot create out.txt: Operation not permitted",
    );
    assert!(hint.is_some_and(|line| line.starts_with("next: ")));
}

/// The live half. Everything above describes the policy; this runs it.
#[cfg(target_os = "macos")]
#[test]
fn a_contained_command_writes_only_where_the_policy_says() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, home) = workspace("write")?;
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside)?;
    let sandbox = Sandbox {
        writable: vec![project.clone()],
        deny_read: vec![home.join(".ssh")],
        deny_write: Vec::new(),
    };

    let (code, output) = run("echo contained > inside.txt", &project, Some(&sandbox))?;
    assert_eq!(code, 0, "a write inside the tree must run: {output}");
    assert_eq!(
        std::fs::read_to_string(project.join("inside.txt"))?.trim(),
        "contained"
    );

    let escape = format!("echo escaped > {}/outside.txt", outside.display());
    let (code, output) = run(&escape, &project, Some(&sandbox))?;
    assert_ne!(code, 0, "a write outside the tree must fail: {output}");
    assert!(
        !outside.join("outside.txt").exists(),
        "the file must not exist"
    );
    assert!(
        denial_hint(Some(code), &output).is_some(),
        "the failure must read as a sandbox denial: {output}"
    );

    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn a_contained_command_reads_the_tree_but_not_the_keys() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home) = workspace("read")?;
    let sandbox = Sandbox {
        writable: vec![project.clone()],
        deny_read: vec![home.join(".ssh")],
        deny_write: Vec::new(),
    };

    let ordinary = format!("cat {}", home.join("notes.md").display());
    let (code, output) = run(&ordinary, &project, Some(&sandbox))?;
    assert_eq!(code, 0, "an ordinary read still works: {output}");
    assert!(output.contains("ordinary"));

    let key = format!("cat {}", home.join(".ssh/id_rsa").display());
    let (code, output) = run(&key, &project, Some(&sandbox))?;
    assert_ne!(code, 0, "the key must not be readable: {output}");
    assert!(!output.contains("PRIVATE KEY"), "{output}");

    Ok(())
}

/// Egress is off because the base policy denies by default and no network rule
/// is ever added. Skipped when the machine has no network to lose.
#[cfg(target_os = "macos")]
#[test]
fn a_contained_command_has_no_network() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home) = workspace("network")?;
    let probe = "curl -m 5 -sS -o /dev/null https://example.com";
    let (code, _) = run(probe, &project, None)?;
    if code != 0 {
        return Ok(());
    }
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let (code, output) = run(probe, &project, Some(&sandbox))?;
    assert_ne!(code, 0, "the sandbox must refuse egress: {output}");
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture repository is built with the real git a contained command runs"
    )]
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A trunk with one commit and a detached linked worktree beside it, the shape of a lane.
fn linked_worktree(root: &Path) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let trunk = root.join("trunk");
    std::fs::create_dir_all(&trunk)?;
    git(&trunk, &["init", "-q", "-b", "main"])?;
    git(&trunk, &["config", "user.email", "yi@example.com"])?;
    git(&trunk, &["config", "user.name", "yi"])?;
    std::fs::write(trunk.join("README.md"), "trunk\n")?;
    git(&trunk, &["add", "README.md"])?;
    git(&trunk, &["commit", "-q", "-m", "init"])?;
    let lane = root.join("lane");
    git(
        &trunk,
        &["worktree", "add", "-q", "--detach", &lane.to_string_lossy()],
    )?;
    Ok((trunk, lane))
}

#[test]
fn a_linked_worktree_sandbox_writes_its_gitdir_and_common_dir() -> TestResult {
    let (root, _project, home) = workspace("gitdirs")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let sandbox = Sandbox::for_workspace(&lane, &home, None);
    let common = trunk.join(".git").canonicalize()?;
    let gitdir = PathBuf::from(git(&lane, &["rev-parse", "--absolute-git-dir"])?).canonicalize()?;
    let roots: Vec<PathBuf> = sandbox
        .params()
        .into_iter()
        .filter(|(key, _)| key.starts_with("WRITABLE_ROOT_"))
        .map(|(_, path)| path)
        .collect();
    for wanted in [&gitdir, &common] {
        assert!(
            roots.iter().any(|root| root == wanted),
            "{} must be writable, roots: {roots:?}",
            wanted.display()
        );
    }
    Ok(())
}

/// The incident: `git add` in a lane failed on `.git/worktrees/<lane>/index.lock`.
#[cfg(target_os = "macos")]
#[test]
fn a_contained_git_commit_succeeds_in_a_linked_worktree() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, _project, home) = workspace("commit")?;
    let (_trunk, lane) = linked_worktree(&root)?;
    std::fs::write(lane.join("change.txt"), "from the lane\n")?;
    let mut sandbox = Sandbox::for_workspace(&lane, &home, None);
    // The scratch dir sits under tmp, a writable root that would hide the trunk's git dir.
    let scratch = root.canonicalize()?;
    sandbox.writable.retain(|writable| {
        writable
            .canonicalize()
            .is_ok_and(|resolved| resolved.starts_with(&scratch))
            || writable == Path::new("/private/tmp")
    });
    let command = "git checkout -q -b feature && git add -A && \
        GIT_CONFIG_GLOBAL=/dev/null git -c user.email=yi@example.com -c user.name=yi commit -q -m lane";
    let (code, output) = run(command, &lane, Some(&sandbox))?;
    assert_eq!(
        code, 0,
        "a contained commit in a lane must succeed: {output}"
    );
    assert_eq!(git(&lane, &["log", "-1", "--format=%s"])?, "lane");
    Ok(())
}

/// A hook and a config are code git runs on the host, outside any sandbox: making the git dirs
/// writable must not hand a contained command a way out of the sandbox (D205).
#[cfg(target_os = "macos")]
#[test]
fn a_contained_command_cannot_write_a_hook_or_the_config() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, _project, home) = workspace("hooks")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let mut sandbox = Sandbox::for_workspace(&lane, &home, None);
    let scratch = root.canonicalize()?;
    sandbox.writable.retain(|writable| {
        writable
            .canonicalize()
            .is_ok_and(|resolved| resolved.starts_with(&scratch))
            || writable == Path::new("/private/tmp")
    });
    let common = trunk.join(".git");
    for target in [
        common.join("hooks/post-commit"),
        common.join("config"),
        common.join("worktrees/lane/commondir"),
    ] {
        let command = format!("printf x >> {}", target.display());
        let (code, output) = run(&command, &lane, Some(&sandbox))?;
        assert_ne!(
            code,
            0,
            "{} must stay out of reach: {output}",
            target.display()
        );
    }
    // The point of the grant is still met: the index and refs are writable.
    std::fs::write(lane.join("change.txt"), "from the lane\n")?;
    let (code, output) = run("git add -A", &lane, Some(&sandbox))?;
    assert_eq!(code, 0, "a contained `git add` still works: {output}");
    Ok(())
}
