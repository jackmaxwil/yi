#![cfg(target_os = "macos")]

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_tools::{
    Run, Sandbox, SandboxRefusal, ToolContext, denial_hint, run_or_background, sandbox_refusal,
};

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
    let context = ToolContext::new(cwd.to_path_buf());
    let timeout = std::time::Duration::from_secs(120);
    match run_or_background(command, &context, None, timeout, sandbox, None)? {
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
    let command = prefix.iter().skip_while(|arg| *arg != "--").nth(1);
    assert_eq!(
        command.map(String::as_str),
        Some("/usr/bin/env"),
        "the kernel starts through `env -u`, which drops secret-named variables"
    );
    Ok(())
}

#[test]
fn a_denial_is_only_claimed_when_the_output_says_so() {
    let cwd = Path::new("/work");
    let sandbox = Sandbox {
        writable: vec![cwd.to_path_buf()],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    };
    let hint = |code, output: &str, command: &str| {
        sandbox_refusal(&sandbox, cwd, Some(code), output, command)
            .map(|refusal| denial_hint(&refusal))
            .unwrap_or_default()
    };
    assert!(hint(0, "operation not permitted", "touch x").is_empty());
    assert!(hint(127, "command not found", "nope").is_empty());
    assert!(hint(1, "assertion failed", "cargo nextest run").is_empty());
    let scoped = hint(
        1,
        "sh: cannot create out.txt: Operation not permitted",
        "cd /x && cargo fmt 2>&1 | head",
    );
    assert!(scoped.starts_with("next: "), "{scoped}");
    assert!(
        scoped.contains("`cargo fmt` now needs permission") && !scoped.contains("same command"),
        "no denied path: the hint names the refused scope, as the broker remembers it: {scoped}"
    );
    // `/dev/null` is the base policy's to grant, and a path inside the tree is no sandbox denial.
    let redirected = hint(
        1,
        "bash: /outside/y: Operation not permitted",
        "ls 2>/dev/null; echo y > /outside/y",
    );
    assert!(
        redirected.contains("refused writing `/outside/y`"),
        "{redirected}"
    );
    let inside = hint(
        1,
        "chmod: /work/x: Operation not permitted",
        "chmod 0 /work/x",
    );
    assert!(!inside.contains("refused writing"), "{inside}");
}

/// A sandbox over `/work` with a project under HOME, as a worktree in the home directory has.
fn home_project() -> Result<(Sandbox, PathBuf), Box<dyn Error>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let sandbox = Sandbox {
        writable: vec![PathBuf::from("/work"), home.join("project")],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    };
    Ok((sandbox, home))
}

/// The model's second try at a refused home write spells it `$HOME`, quotes it inside python or
/// hangs it on a flag; each must ask, where a whole-word match let it run contained again.
#[test]
fn a_retry_asks_in_every_spelling_of_the_refused_directory() -> TestResult {
    let (sandbox, home) = home_project()?;
    let cwd = Path::new("/work");
    let refused = SandboxRefusal::Path(home.join("yidog_probe"));
    let abs = home.display();
    for retry in [
        format!("echo y > {abs}/yidog_probe"),
        "echo y > ~/yidog_probe".to_owned(),
        "echo y > $HOME/yidog_probe".to_owned(),
        "echo y > \"${HOME}\"/yidog_probe".to_owned(),
        format!("python3 -c \"open('{abs}/yidog_probe','w').write('y')\""),
        format!("cargo build --target-dir={abs}/target"),
        format!("P={abs}/yidog_probe; echo y > $P"),
        "cd ~ && ./write-here".to_owned(),
    ] {
        assert!(refused.retried_by(&sandbox, cwd, &retry), "{retry}");
    }
    for unrelated in [
        "yes | head -1".to_owned(),
        "echo hi > /work/x".to_owned(),
        format!("./check {abs}/project/src/lib.rs"),
        "git log HEAD~1".to_owned(),
    ] {
        assert!(
            !refused.retried_by(&sandbox, cwd, &unrelated),
            "{unrelated}"
        );
    }
    Ok(())
}

/// A refusal outside home asks for a write there and never for a read: `cat /etc/hosts` stays
/// contained after `/etc/yi-probe` was refused.
#[test]
fn a_refused_system_path_asks_for_writes_there_and_never_for_reads() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let cwd = Path::new("/work");
    let refused = SandboxRefusal::Path(PathBuf::from("/etc/yi-probe"));
    assert!(refused.retried_by(&sandbox, cwd, "echo y > /etc/yi-other"));
    for read in [
        "cat /etc/hosts",
        "ls /etc | head -3",
        "./tool --config=/etc/tool.conf",
    ] {
        assert!(!refused.retried_by(&sandbox, cwd, read), "{read}");
    }
    Ok(())
}

/// Printed text is no refusal: the phrase echoed, grep's hits in the SDK's errno header, and a
/// privacy read refusal from `find` behind an exit-0 pipe all name no refused write.
#[test]
fn only_an_errno_line_of_a_failed_command_names_a_refused_path() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let cwd = Path::new("/work");
    let sdk = "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk/usr/include/sys/errno.h";
    let grep = format!(
        "{sdk}:88:#define EPERM           1               /* Operation not permitted */\n\
         {sdk}:101:#define EACCES          13              /* Permission denied */\n"
    );
    let cases = [
        (
            0,
            "y\n/ permission denied\n".to_owned(),
            "yes | head -1; echo '/ permission denied'",
        ),
        (
            1,
            "y\n/ permission denied\n".to_owned(),
            "yes | head -1; echo '/ permission denied'; false",
        ),
        (
            0,
            grep.clone(),
            "yes | head -1; grep -rn 'Permission denied' $SDK/usr/include/sys/",
        ),
        (
            1,
            grep,
            "yes | head -1; grep -rn 'Permission denied' $SDK/usr/include/sys/; false",
        ),
        (
            0,
            "/Users/dev/Library/Mail\nfind: /Users/dev/Library/Mail: Operation not permitted\n"
                .to_owned(),
            "find /Users/dev/Library/Mail -maxdepth 1 2>&1 | head -2",
        ),
    ];
    for (code, output, command) in cases {
        let found = sandbox_refusal(&sandbox, cwd, Some(code), &output, command);
        assert!(
            !matches!(found, Some(SandboxRefusal::Path(_))),
            "{command}: {found:?}"
        );
    }
    Ok(())
}

/// The base policy grants `/dev/null`, `/dev/ptmx` and the ttys, nothing else under `/dev`: a
/// refused `> /dev/stderr` is named, not blamed on the `yes` before it.
#[test]
fn a_refused_device_write_is_named() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let found = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(1),
        "y\nbash: line 1: /dev/stderr: Operation not permitted\n",
        "yes | head -1; echo hi > /dev/stderr",
    );
    assert_eq!(
        found,
        Some(SandboxRefusal::Path(PathBuf::from("/dev/stderr")))
    );
    Ok(())
}

/// The plan's case: `git worktree add … | tail -3` exits 0, and git quotes the path it could not
/// create, so the worktree operand is the refused write.
#[test]
fn a_refused_worktree_behind_a_zero_exit_pipe_is_named() -> TestResult {
    let (sandbox, home) = home_project()?;
    let worktree = home.join("yidog_wt");
    let found = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(0),
        &format!(
            "fatal: could not create leading directories of '{}/.git': Operation not permitted\n",
            worktree.display()
        ),
        &format!("git worktree add -q {} 2>&1 | tail -3", worktree.display()),
    );
    assert_eq!(found, Some(SandboxRefusal::Path(worktree)));
    Ok(())
}

/// The broker reads the refusal the bash tool found from its result details, so both kinds
/// survive the trip, and a result with no refusal carries none.
#[test]
fn a_refusal_round_trips_through_the_result_details() {
    for refusal in [
        SandboxRefusal::Path(PathBuf::from("/outside/y")),
        SandboxRefusal::Scopes(vec!["curl".to_owned(), "git worktree".to_owned()]),
    ] {
        assert_eq!(SandboxRefusal::from_json(&refusal.to_json()), Some(refusal));
    }
    assert_eq!(SandboxRefusal::from_json(&serde_json::Value::Null), None);
    // An empty or relative path would sit above every path, so every outside write would ask.
    for path in ["", "relative/y"] {
        let detail = serde_json::json!({ "path": path });
        assert_eq!(SandboxRefusal::from_json(&detail), None, "{path:?}");
    }
}

/// A `~` counts as home only where a shell expands it, at a word's start: backup files, a
/// `sed` delimiter and a printed tilde are no home path, even after a refusal directly in home.
#[test]
fn a_tilde_inside_a_word_is_not_home() -> TestResult {
    let (sandbox, home) = home_project()?;
    let refused = SandboxRefusal::Path(home.join("yidog_probe"));
    for command in [
        "rm *~",
        "sed -i 's~a~b~' notes.txt",
        "python3 -c \"print('~')\"",
        "echo ~'quoted'",
    ] {
        assert!(
            !refused.retried_by(&sandbox, Path::new("/work"), command),
            "{command}"
        );
    }
    Ok(())
}

/// The hint says what asks next: in home a call naming a path there, elsewhere only a write.
#[test]
fn the_hint_words_the_rule_the_broker_keeps() -> TestResult {
    let (_sandbox, home) = home_project()?;
    let in_home = denial_hint(&SandboxRefusal::Path(home.join("yidog_probe")));
    assert!(
        in_home.contains(&format!(
            "the next call naming a path under `{}`",
            home.display()
        )),
        "{in_home}"
    );
    let outside = denial_hint(&SandboxRefusal::Path(PathBuf::from("/etc/yi-probe")));
    assert!(
        outside.contains("the next call writing under `/etc`"),
        "{outside}"
    );
    Ok(())
}

/// Rust's io errors (rustfmt, uv) and node's EPERM name the refused path too; the lines are
/// theirs under Seatbelt, the home directory renamed.
#[test]
fn rust_and_node_errno_lines_name_the_refused_path() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let cases = [
        (
            "rustfmt /Users/dev/fmt.rs",
            "Error writing files: io error: /Users/dev/fmt.rs: Operation not permitted (os error 1)\n",
            "/Users/dev/fmt.rs",
        ),
        (
            "uv venv -q /Users/dev/venv",
            "error: Failed to initialize cache at `/Users/dev/.cache/uv`\n  Caused by: failed to open file `/Users/dev/.cache/uv/sdists-v9/.git`: Operation not permitted (os error 1)\n",
            "/Users/dev/.cache/uv/sdists-v9/.git",
        ),
        (
            "node -e \"require('fs').writeFileSync('/Users/dev/out','y')\"",
            "node:fs:2482\n    return binding.writeFileUtf8(\n\nError: EPERM: operation not permitted, open '/Users/dev/out'\n    at Object.writeFileSync (node:fs:2482:20)\n",
            "/Users/dev/out",
        ),
    ];
    for (command, output, path) in cases {
        let found = sandbox_refusal(&sandbox, Path::new("/work"), Some(1), output, command);
        assert_eq!(
            found,
            Some(SandboxRefusal::Path(PathBuf::from(path))),
            "{command}"
        );
    }
    Ok(())
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
        loopback: false,
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
        sandbox_refusal(&sandbox, &project, Some(code), &output, &escape).is_some(),
        "the failure must read as a sandbox denial: {output}"
    );

    Ok(())
}

#[cfg(target_os = "macos")]
/// MCP OAuth tokens sit under `~/.yi/mcp/tokens` and provider logins under
/// `~/.yi/providers/tokens` (#887). The base profile hides both from every contained command,
/// the bash tool's and the document converter's included, not only the kernel's.
#[test]
fn a_contained_command_cannot_read_yi_tokens() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home) = workspace("tokens")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);

    let ordinary = format!("cat {}", home.join("notes.md").display());
    let (code, output) = run(&ordinary, &project, Some(&sandbox))?;
    assert_eq!(code, 0, "an ordinary read still works: {output}");

    for (store, name) in [
        (".yi/mcp/tokens", "default_mcp.example.com.json"),
        (".yi/providers/tokens", "openai.json"),
    ] {
        let token = home.join(store).join(name);
        std::fs::create_dir_all(home.join(store))?;
        std::fs::write(&token, "BEARER SECRET")?;
        for command in [format!("cat {}", token.display()), format!("ls {store}")] {
            let (code, output) = run(&command, &home, Some(&sandbox))?;
            assert_ne!(code, 0, "`{command}` must fail: {output}");
            let leaked =
                output.contains("BEARER SECRET") || output.lines().any(|line| line == name);
            assert!(!leaked, "`{command}` leaks: {output}");
        }
    }
    Ok(())
}

/// Runs each command contained from `cwd` and returns those whose output carries `secret`.
fn leaks(
    commands: &[String],
    cwd: &Path,
    sandbox: &Sandbox,
    secret: &str,
) -> Result<Vec<String>, String> {
    let mut leaked = Vec::new();
    for command in commands {
        let (_, output) = run(command, cwd, Some(sandbox))?;
        if output.contains(secret) {
            leaked.push(command.clone());
        }
    }
    Ok(leaked)
}

/// #598: keys kept outside yi's token stores were readable from every sandbox. The scratch HOME
/// sits under a writable temp root, so renaming a store's parent is a live way around too.
#[test]
fn a_contained_command_reads_no_key_by_any_spelling() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home) = workspace("keys")?;
    let keys = [
        ".netrc",
        ".git-credentials",
        ".npmrc",
        ".pypirc",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".config/gh/hosts.yml",
        ".config/fgj/config.yaml",
        ".config/gcloud/credentials.db",
        ".yi/oauth/acme.json",
    ];
    for key in keys {
        let path = home.join(key);
        std::fs::create_dir_all(path.parent().ok_or("a key has a parent")?)?;
        std::fs::write(&path, "KEY-598")?;
    }
    std::os::unix::fs::symlink(home.join(".config"), project.join("cfg"))?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let home_text = home.display();
    let mut commands: Vec<String> = keys
        .iter()
        .flat_map(|key| {
            let upper = key.to_uppercase();
            [
                format!("cat {home_text}/{key}"),
                format!("cat {home_text}/{upper}"),
            ]
        })
        .collect();
    commands.extend([
        "cat cfg/gh/hosts.yml".to_owned(),
        format!("ln {home_text}/.netrc hard; cat hard"),
        format!("mv {home_text}/.config {home_text}/moved; cat {home_text}/moved/gh/hosts.yml"),
        format!("mv {home_text}/.yi {home_text}/yi2; cat {home_text}/yi2/oauth/acme.json"),
    ]);
    let leaked = leaks(&commands, &project, &sandbox, "KEY-598")?;
    assert!(leaked.is_empty(), "a key was read: {leaked:#?}");
    Ok(())
}

/// #889: a wall spelled through a link bound nothing, since Seatbelt matches the real path, and
/// a walled file inside the writable tree left by a renamed parent or a hard link.
#[test]
fn a_walled_path_is_unreadable_however_either_side_spells_it() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home) = workspace("wall")?;
    std::fs::create_dir_all(project.join("a/secrets"))?;
    std::fs::write(project.join("a/secrets/key.txt"), "WALLED-598")?;
    std::os::unix::fs::symlink(project.join("a"), project.join("via"))?;
    std::fs::create_dir_all(project.join("a/locked"))?;
    let mut sandbox = Sandbox::for_workspace(&project, &home, None);
    sandbox.deny_read.push(project.join("via/secrets"));
    sandbox.deny_write.push(project.join("via/locked"));
    assert!(
        sandbox.denies_write(&project.join("via/secrets/planted")),
        "the refusal hint must name a write under a read-walled path"
    );
    let commands = [
        "cat a/secrets/key.txt",
        "cat A/SECRETS/KEY.TXT",
        "cat via/secrets/key.txt",
        "ln a/secrets/key.txt hard; cat hard",
        "mv a/secrets out; cat out/key.txt",
        "mv a b; cat b/secrets/key.txt",
    ]
    .map(str::to_owned);
    let leaked = leaks(&commands, &project, &sandbox, "WALLED-598")?;
    assert!(leaked.is_empty(), "a walled file was read: {leaked:#?}");
    for target in ["a/secrets/planted", "a/locked/planted"] {
        let (code, output) = run(&format!("echo x > {target}"), &project, Some(&sandbox))?;
        assert!(
            code != 0 && !project.join(target).exists(),
            "{target}: a wall spelled through a link binds writes too: {output}"
        );
    }
    Ok(())
}

/// #906: `*_API_KEY` and the like reached every contained command's environment.
#[test]
fn a_contained_command_inherits_no_secret_variable() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let secrets = [
        "YI_598_API_KEY",
        "YI_598_TOKEN",
        "YI_598_CLIENT_SECRET",
        "YI_598_PGPASSWORD",
        "YI_598_CREDENTIALS",
        "DATABASE_URL",
        "GITHUB_PAT",
        "BW_SESSION",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ];
    let settings = [
        "TOKENIZERS_PARALLELISM",
        "PYTHON_KEYRING_BACKEND",
        "PASSWORD_STORE_DIR",
    ];
    for name in secrets.iter().chain(&settings) {
        // SAFETY: nextest runs each test in a process of its own, so no thread reads the env.
        unsafe { std::env::set_var(name, format!("ENV-598-{name}")) };
    }
    let (_root, project, home) = workspace("env")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    // The messages never print the environment: it holds whatever keys the runner has.
    let (code, contained) = run("env", &project, Some(&sandbox))?;
    let seen = |name: &str| contained.contains(&format!("ENV-598-{name}"));
    let leaked: Vec<&str> = secrets.into_iter().filter(|name| seen(name)).collect();
    let stripped: Vec<&str> = settings.into_iter().filter(|name| !seen(name)).collect();
    assert!(
        code == 0 && contained.contains("PATH=") && leaked.is_empty() && stripped.is_empty(),
        "a contained `env` exited {code}, saw {leaked:?}, lost the settings {stripped:?}"
    );
    Ok(())
}

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
        loopback: false,
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
