#![cfg(target_os = "macos")]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_tools::{
    Run, Sandbox, SandboxRefusal, Tool, ToolContext, denial_hint, run_or_background,
    sandbox_refusal,
};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

use crate::seatbelt::seatbelt;
fn d_bindings(args: &[String]) -> Vec<(&str, &str)> {
    args.iter()
        .filter_map(|arg| arg.strip_prefix("-D")?.split_once('='))
        .collect()
}

fn workspace(tag: &str) -> Result<(Scratch, PathBuf, PathBuf), Box<dyn Error>> {
    workspace_at(&std::env::temp_dir(), tag)
}

fn workspace_at(base: &Path, tag: &str) -> Result<(Scratch, PathBuf, PathBuf), Box<dyn Error>> {
    let root = Scratch::new_at(base, &format!("yi-sandbox-{tag}"))?;
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
    let (program, args) = sandbox.wrap("sh", &["-c", "ls"]);
    let policy = args.get(1).ok_or("the policy follows -p")?;
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
        policy.contains("(allow network-outbound (remote ip \"localhost:*\"))")
            && !policy.contains("\"*:"),
        "loopback is the one network, for every profile: {policy}"
    );

    let params = d_bindings(&args);
    assert!(params.iter().any(|(key, _)| *key == "WRITABLE_ROOT_0"));
    assert!(params.iter().any(|(key, _)| *key == "DENY_READ_0"));
    assert!(
        params.iter().any(|(_, path)| *path == "/repo"),
        "the working tree is writable"
    );
    assert!(
        params.iter().any(|(_, path)| *path == "/home/user/.ssh"),
        "credential stores are read-denied"
    );

    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert_eq!(args.first().map(String::as_str), Some("-p"));
    let tail: Vec<&str> = args.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(tail, vec!["ls", "-c", "sh"], "the command comes last");
    assert!(args.iter().any(|arg| arg == "--"));
    assert!(args.iter().any(|arg| arg.starts_with("-DWRITABLE_ROOT_0=")));

    let (_, offline) = sandbox.wrap_offline("sh", &[]);
    assert!(
        offline
            .get(1)
            .is_some_and(|policy| !policy.contains("network")),
        "the offline profile has no network rule at all"
    );
    assert!(
        args.iter().any(|arg| arg.starts_with("JAVA_TOOL_OPTIONS=")
            && arg.ends_with("-Djava.net.preferIPv4Stack=true")),
        "a JVM gets IPv4 sockets, which reach loopback"
    );

    let own = Path::new("/home/user/.yi/kernel-connections/1-0");
    let (program, prefix) = sandbox.kernel_prefix(own, &[]);
    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert!(
        prefix
            .iter()
            .any(|arg| arg == "-DOWN_CONNECTION=/home/user/.yi/kernel-connections/1-0")
            && !args.iter().any(|arg| arg.contains("OWN_CONNECTION")),
        "only the kernel's own prefix re-allows its connection file"
    );
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
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
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
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
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
#[test]
fn a_contained_command_writes_only_where_the_policy_says() -> TestResult {
    seatbelt()?;
    let (root, project, home) = workspace("write")?;
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside)?;
    let sandbox = Sandbox {
        writable: vec![project.clone()],
        deny_read: vec![home.join(".ssh")],
        deny_write: Vec::new(),
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
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

/// MCP OAuth tokens sit under `~/.yi/mcp/tokens` and provider logins under
/// `~/.yi/providers/tokens` (#887). The base profile hides both from every contained command,
/// the bash tool's and the document converter's included, not only the kernel's.
#[test]
fn a_contained_command_cannot_read_yi_tokens() -> TestResult {
    seatbelt()?;
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
    seatbelt()?;
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
        ".password-store/github.gpg",
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
    seatbelt()?;
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
    seatbelt()?;
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
        "PASSWORD_STORE_DIR",
        "SSH_AUTH_SOCK",
    ];
    let settings = ["TOKENIZERS_PARALLELISM", "PYTHON_KEYRING_BACKEND"];
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
    seatbelt()?;
    let (_root, project, home) = workspace("read")?;
    let sandbox = Sandbox {
        writable: vec![project.clone()],
        deny_read: vec![home.join(".ssh")],
        deny_write: Vec::new(),
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
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

/// #889: under a denied read root, a spared writable root (a walled kernel's own dir, the family
/// board) reads and takes writes; a spared path no root grants (its own spill dir) only reads; a
/// denial inside a spare, read or write, holds (#1000 review F1, F2), and an aliased spare matches.
#[test]
fn a_spared_writable_root_takes_writes_and_a_spared_path_only_reads() -> TestResult {
    seatbelt()?;
    // Under `/tmp` whatever TMPDIR says: macOS always spells this root two ways.
    let (_root, project, _home) = workspace_at(Path::new("/tmp"), "spared")?;
    let store = project.join("store");
    let (state, spill) = (store.join("state"), store.join("spill"));
    for (dir, text) in [(&state, "SPARED"), (&spill, "SPARED"), (&store, "WALLED")] {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("f"), text)?;
    }
    std::fs::create_dir_all(state.join("secret"))?;
    // The spare names the root by another spelling (`/tmp` for `/private/tmp`), as a link may.
    let spelled = Path::new("/").join(state.strip_prefix("/private")?);
    assert_eq!(spelled.canonicalize()?, state, "one root, two spellings");
    std::fs::write(state.join("secret/f"), "WALLED")?;
    let sandbox = Sandbox {
        writable: vec![project.clone(), state.clone()],
        deny_read: vec![store.clone(), state.join("secret")],
        deny_write: vec![state.join("locked")],
        sealed: Vec::new(),
        host_owned: Vec::new(),
        spared: vec![spelled, spill],
    };
    let script = "cat store/state/f store/spill/f store/f store/state/secret/f; \
        echo x > store/state/new && echo wrote-state; echo x > store/spill/new && echo wrote-spill; \
        echo x > store/state/locked && echo wrote-locked; echo x > store/state/secret/g && echo wrote-secret";
    let (_, output) = run(script, &project, Some(&sandbox))?;
    let seen = (
        output.matches("SPARED").count(),
        output.contains("WALLED"),
        output.contains("wrote-state"),
        output.contains("wrote-spill") || output.contains("wrote-locked"),
        output.contains("wrote-secret"),
    );
    assert_eq!(seen, (2, false, true, false, false), "{output}");
    Ok(())
}

/// What a contained script reports, one word per probe: loopback works both ways, and nothing
/// past it answers, however the address is spelled. Every refusal must be EPERM, the sandbox's
/// errno, so a machine with no network cannot pass it by timing out.
const NETWORK_PROBE: &str = r#"
import socket, sys
host_port, unix_path = int(sys.argv[1]), sys.argv[2]
def probe(word, attempt):
    try:
        attempt()
        print(word + '-ok')
    except PermissionError:
        print(word + '-refused')
    except OSError as error:
        print(word + '-' + type(error).__name__)
def loop():
    server = socket.socket(); server.bind(('127.0.0.1', 0)); server.listen(1)
    client = socket.create_connection(server.getsockname(), 3); server.accept()[0].send(b'x')
    assert client.recv(1) == b'x'
def connect(family, address):
    return lambda: socket.socket(family).connect(address)
def host():
    assert socket.create_connection(('127.0.0.1', host_port), 3).recv(4) == b'HOST'
def unix():
    sock = socket.socket(socket.AF_UNIX); sock.connect(unix_path)
probe('loop', loop)
probe('host', host)
probe('v4', connect(socket.AF_INET, ('1.1.1.1', 443)))
probe('mapped', connect(socket.AF_INET6, ('::ffff:1.1.1.1', 443)))
probe('v6', connect(socket.AF_INET6, ('2606:4700:4700::1111', 443)))
probe('udp', lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(b'x', ('1.1.1.1', 53)))
probe('unix', unix)
try:
    socket.getaddrinfo('example.com', 443)
    print('dns-ok')
except socket.gaierror:
    print('dns-refused')
"#;

/// #599: every contained spawn gets loopback, so a test suite that binds 127.0.0.1 runs; the
/// rest of the network stays off, and so do unix sockets (the daemon's, docker's, ssh-agent's),
/// even one inside a writable root.
#[test]
fn a_contained_command_has_loopback_and_no_other_network() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("network")?;
    std::fs::write(project.join("probe.py"), NETWORK_PROBE)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let unix_path = project.join("host.sock");
    let unix = std::os::unix::net::UnixListener::bind(&unix_path)?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = std::io::Write::write_all(&mut &stream, b"HOST");
        }
    });
    std::thread::spawn(move || for _ in unix.incoming() {});
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let command = format!("/usr/bin/python3 probe.py {port} {}", unix_path.display());
    let (code, output) = run(&command, &project, Some(&sandbox))?;
    let words: Vec<&str> = (output.lines())
        .filter(|line| line.contains('-') && !line.contains(' '))
        .collect();
    assert_eq!(
        (code, words),
        (
            0,
            vec![
                "loop-ok",
                "host-ok",
                "v4-refused",
                "mapped-refused",
                "v6-refused",
                "udp-refused",
                "unix-refused",
                "dns-refused"
            ]
        ),
        "{output}"
    );
    Ok(())
}

/// #599: past loopback a contained run fails at the resolver, since the base policy grants no
/// DNS lookup; curl says `Could not resolve host`, Python `nodename nor servname`. Either reads
/// as the sandbox's refusal, not as the network being down.
#[test]
fn a_contained_resolver_failure_reads_as_the_sandbox() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("resolver")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    for command in [
        "curl -sS -m 5 -o /dev/null https://example.com",
        "/usr/bin/python3 -c \"import socket; socket.getaddrinfo('example.com', 443)\"",
    ] {
        let (code, output) = run(command, &project, Some(&sandbox))?;
        let hint = sandbox_refusal(&sandbox, &project, Some(code), &output, command)
            .map(|refusal| denial_hint(&refusal))
            .unwrap_or_default();
        assert!(
            code != 0 && hint.contains("only through its proxy on 127.0.0.1"),
            "`{command}` exited {code} with no sandbox hint: {output}"
        );
    }
    Ok(())
}

/// Review of #925 (F3): a JVM opens dual-stack sockets and dials 127.0.0.1 as `::ffff:127.0.0.1`,
/// which Seatbelt refuses, so Gradle, Maven and sbt lost loopback. A JDK is optional, so this
/// rides the ignored lane, where a missing one fails.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_contained_jvm_reaches_loopback() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("jvm")?;
    let (code, java_home) = run("/usr/libexec/java_home", &project, None)?;
    if code != 0 {
        return Err("no JDK: /usr/libexec/java_home found none".into());
    }
    std::fs::write(
        project.join("Probe.java"),
        "import java.net.*;\npublic class Probe {\n  public static void main(String[] args) throws Exception {\n    for (String host : new String[] {\"127.0.0.1\", \"localhost\"}) {\n      try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getByName(host));\n           Socket client = new Socket(host, server.getLocalPort())) {\n        server.accept().close();\n        System.out.println(\"jvm-\" + host + \"-ok\");\n      }\n    }\n  }\n}\n",
    )?;
    let java = format!("{}/bin/java", java_home.trim());
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let (code, output) = run(&format!("'{java}' Probe.java"), &project, Some(&sandbox))?;
    assert!(
        code == 0 && output.contains("jvm-127.0.0.1-ok") && output.contains("jvm-localhost-ok"),
        "{output}"
    );
    Ok(())
}

/// Plan row F's demo: a contained `cargo test` whose test binds 127.0.0.1 passes.
#[test]
fn a_contained_cargo_test_binds_loopback() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("cargo")?;
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        project.join("src/lib.rs"),
        "#[test]\nfn binds_loopback() {\n    use std::io::{Read, Write};\n    let server = std::net::TcpListener::bind(\"127.0.0.1:0\").unwrap();\n    let mut client = std::net::TcpStream::connect(server.local_addr().unwrap()).unwrap();\n    server.accept().unwrap().0.write_all(b\"ok\").unwrap();\n    let mut got = [0; 2];\n    client.read_exact(&mut got).unwrap();\n    assert_eq!(&got, b\"ok\");\n}\n",
    )?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let (code, output) = run(
        &format!("'{cargo}' test --offline -q"),
        &project,
        Some(&sandbox),
    )?;
    assert!(code == 0 && output.contains("1 passed"), "{output}");
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
fn a_linked_worktree_sandbox_writes_its_gitdir_and_objects_not_the_common_dir() -> TestResult {
    let (root, _project, home) = workspace("gitdirs")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let sandbox = Sandbox::for_workspace(&lane, &home, None);
    let common = trunk.join(".git").canonicalize()?;
    let gitdir = PathBuf::from(git(&lane, &["rev-parse", "--absolute-git-dir"])?).canonicalize()?;
    let (_, args) = sandbox.wrap("true", &[]);
    let roots: Vec<PathBuf> = d_bindings(&args)
        .into_iter()
        .filter(|(key, _)| key.starts_with("WRITABLE_ROOT_"))
        .map(|(_, path)| PathBuf::from(path))
        .collect();
    for wanted in [&gitdir, &common.join("objects")] {
        assert!(
            roots.iter().any(|root| root == wanted),
            "{} must be writable, roots: {roots:?}",
            wanted.display()
        );
    }
    assert!(!roots.contains(&common), "{roots:?}");
    Ok(())
}

/// The incident: `git add` in a lane failed on `.git/worktrees/<lane>/index.lock`.
#[test]
fn a_contained_git_commit_succeeds_in_a_linked_worktree() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("commit")?;
    let (_trunk, lane) = linked_worktree(&root)?;
    std::fs::write(lane.join("change.txt"), "from the lane\n")?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    let command = "git add -A && \
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
#[test]
fn a_contained_command_cannot_write_a_hook_or_the_config() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("hooks")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let sandbox = lane_profile(&root, &lane, &home)?;
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

/// The lane profile with tmp's blanket grant dropped: the scratch dir sits under tmp, a writable
/// root that would hide the trunk's git dir.
fn lane_profile(root: &Path, lane: &Path, home: &Path) -> Result<Sandbox, Box<dyn Error>> {
    let mut sandbox = Sandbox::for_workspace(lane, home, None);
    let scratch = root.canonicalize()?;
    sandbox.writable.retain(|writable| {
        (writable.ancestors())
            .find_map(|dir| dir.canonicalize().ok())
            .is_some_and(|resolved| resolved.starts_with(&scratch))
            && writable
                .canonicalize()
                .map_or(true, |resolved| resolved != scratch)
    });
    Ok(sandbox)
}

/// A script that leaves `marker` when git runs it, the shape of a planted `core.fsmonitor`.
fn marker_script(root: &Path, marker: &Path) -> Result<PathBuf, Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let script = root.join("fsmonitor.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display()))?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    Ok(script)
}

/// Incident: `config.worktree` was writable, and with `extensions.worktreeConfig` on, as this
/// repository has it, the host's next `git status` ran the `core.fsmonitor` planted there.
#[test]
fn a_planted_worktree_config_never_runs_on_the_host() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("worktree-config")?;
    let (trunk, lane) = linked_worktree(&root)?;
    git(&trunk, &["config", "extensions.worktreeConfig", "true"])?;
    let marker = root.join("ran-on-host");
    let script = marker_script(&root, &marker)?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    let admin = git(&lane, &["rev-parse", "--absolute-git-dir"])?;
    let plant = format!(
        "printf '[core]\\n\\tfsmonitor = {}\\n' >> {admin}/config.worktree",
        script.display()
    );
    let (code, output) = run(&plant, &lane, Some(&sandbox))?;
    assert_ne!(code, 0, "the write must be refused: {output}");
    std::fs::write(lane.join("touched.txt"), "x\n")?;
    git(&lane, &["status", "--porcelain"])?;
    assert!(!marker.exists(), "the planted fsmonitor ran on the host");
    Ok(())
}

/// A repointed `.git` makes the host's git read a config the contained process wrote.
#[test]
fn a_repointed_dot_git_never_runs_on_the_host() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("pointer")?;
    let (_trunk, lane) = linked_worktree(&root)?;
    let marker = root.join("ran-on-host");
    let script = marker_script(&root, &marker)?;
    let evil = root.join("evil");
    std::fs::create_dir_all(&evil)?;
    git(&evil, &["init", "-q", "-b", "main"])?;
    git(
        &evil,
        &["config", "core.fsmonitor", &script.to_string_lossy()],
    )?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    let plant = format!("printf 'gitdir: {}/.git\\n' > .git", evil.display());
    let (code, output) = run(&plant, &lane, Some(&sandbox))?;
    assert_ne!(code, 0, "the pointer must stay out of reach: {output}");
    git(&lane, &["status", "--porcelain"])?;
    assert!(!marker.exists(), "the planted fsmonitor ran on the host");
    Ok(())
}

/// Every other file a git dir holds that the host's git reads as authority: attributes pick merge
/// drivers, a submodule's config runs like the top one, an alternate adds objects unseen.
#[test]
fn a_contained_command_cannot_write_what_host_git_reads() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("host-read")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let common = trunk.join(".git");
    std::fs::create_dir_all(common.join("modules/nested/sub"))?;
    std::fs::write(
        common.join("modules/nested/sub/HEAD"),
        "ref: refs/heads/main\n",
    )?;
    std::fs::write(common.join("modules/nested/sub/config"), "")?;
    std::fs::create_dir_all(common.join("refs/remotes/origin"))?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    for target in [
        common.join("config.worktree"),
        common.join("info/attributes"),
        common.join("info/exclude"),
        common.join("modules/nested/sub/config"),
        common.join("objects/info/alternates"),
        common.join("refs/remotes/origin/HEAD"),
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
    // A lane holds its own branch and nothing shared with its siblings, modules included; a main
    // checkout's work inside a submodule still commits.
    let index = common.join("modules/nested/sub/index");
    let write = format!("printf x > {}", index.display());
    let (code, output) = run(&write, &lane, Some(&sandbox))?;
    assert_ne!(code, 0, "a lane reached a shared module: {output}");
    let trunk = common.parent().ok_or("no trunk")?;
    let main = lane_profile(&root, trunk, &home)?;
    let (code, output) = run(&write, trunk, Some(&main))?;
    assert_eq!(code, 0, "work inside a submodule still commits: {output}");
    Ok(())
}

/// Incident: the first form of this wall stopped a main checkout reaching a lane and left one lane
/// able to plant `config.worktree` in another, whose host-run status then ran it.
#[test]
fn a_lane_cannot_write_a_sibling_lanes_admin() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("sibling")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let other = root.join("other");
    git(
        &trunk,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &other.to_string_lossy(),
        ],
    )?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    let admin = git(&other, &["rev-parse", "--absolute-git-dir"])?;
    for leaf in ["config.worktree", "gitdir", "commondir", "HEAD"] {
        let command = format!("printf x >> {admin}/{leaf}");
        let (code, output) = run(&command, &lane, Some(&sandbox))?;
        assert_ne!(code, 0, "{leaf} must stay out of reach: {output}");
    }
    let own = git(&lane, &["rev-parse", "--absolute-git-dir"])?;
    let (code, output) = run(&format!("printf x > {own}/note"), &lane, Some(&sandbox))?;
    assert_eq!(code, 0, "its own admin stays writable: {output}");
    Ok(())
}

/// The wall sits on the directories that hold admin dirs and modules, never on the names one
/// build found there: a sibling lane or a submodule created after the profile was still writable.
#[test]
fn a_seal_closes_an_admin_dir_or_a_module_the_build_never_saw() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("sealed")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let common = trunk.join(".git").canonicalize()?;
    let module = common.join("modules/early");
    std::fs::create_dir_all(module.join("hooks"))?;
    std::fs::write(module.join("HEAD"), "ref: refs/heads/main\n")?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    let admin = common.join("worktrees/later");
    std::fs::create_dir_all(admin.join("hooks"))?;
    std::fs::create_dir_all(module.join("modules/later/hooks"))?;
    for target in [
        admin.join("config.worktree"),
        admin.join("gitdir"),
        admin.join("HEAD"),
        module.join("modules/later/config"),
        module.join("modules/later/hooks/post-checkout"),
    ] {
        std::fs::write(&target, "")?;
        let (code, output) = run(
            &format!("printf x >> {}", target.display()),
            &lane,
            Some(&sandbox),
        )?;
        assert_ne!(
            code,
            0,
            "{} must stay out of reach: {output}",
            target.display()
        );
    }
    // A module the build did open keeps its object store in a main checkout, whose git dir is
    // writable; a lane reaches none of it.
    let objects = module.join("objects/x");
    std::fs::create_dir_all(module.join("objects"))?;
    let write = format!("printf x >> {}", objects.display());
    let (code, output) = run(&write, &lane, Some(&sandbox))?;
    assert_ne!(code, 0, "a lane reached a shared module: {output}");
    let trunk = common.parent().ok_or("no trunk")?;
    let main = lane_profile(&root, trunk, &home)?;
    let (code, output) = run(&write, trunk, Some(&main))?;
    assert_eq!(code, 0, "{} stays writable: {output}", objects.display());
    Ok(())
}

/// The write gate's own predicate agrees with the seal the policy states, on the same two
/// directions: a name under a sealed directory is refused, a root the seal opens is not.
#[test]
fn the_write_predicate_seals_what_the_policy_seals() -> TestResult {
    let (root, _project, home) = workspace("seal-predicate")?;
    let (trunk, lane) = linked_worktree(&root)?;
    let sandbox = lane_profile(&root, &lane, &home)?;
    assert!(trunk.join(".git").is_dir());
    let mut sealed = 0;
    for seal in &sandbox.sealed {
        sealed += 1;
        for later in [
            "later/config",
            "later/config.worktree",
            "later/hooks/post-checkout",
        ] {
            let path = seal.denied.join(later);
            assert!(
                sandbox.denies_write(&path),
                "{} must be sealed",
                path.display()
            );
        }
        for open in &seal.open {
            assert!(
                !sandbox.denies_write(&open.join("index")),
                "{} stays writable",
                open.display()
            );
            assert!(
                sandbox.denies_write(&open.join("config.worktree")),
                "{} still loses its host-read leaves",
                open.display()
            );
        }
    }
    assert_eq!(
        sealed, 4,
        "a lane seals worktrees and modules in both git dirs"
    );
    Ok(())
}

/// A checkout that is no linked worktree has no `.git` file to protect, but a planted one in a
/// non-repository cwd would turn the host's next git on, and `worktrees` holds every lane's admin.
#[test]
fn a_contained_command_cannot_plant_a_dot_git_or_reach_another_lanes_admin() -> TestResult {
    seatbelt()?;
    let (root, project, home) = workspace("plant")?;
    let sandbox = lane_profile(&root, &project, &home)?;
    let (code, output) = run("printf 'gitdir: /x\\n' > .git", &project, Some(&sandbox))?;
    assert_ne!(code, 0, "a non-repository's .git must be refused: {output}");

    let (trunk, lane) = linked_worktree(&root)?;
    let sandbox = lane_profile(&root, &trunk, &home)?;
    let admin = git(&lane, &["rev-parse", "--absolute-git-dir"])?;
    let command = format!("printf x >> {admin}/gitdir.note");
    let (code, output) = run(&command, &trunk, Some(&sandbox))?;
    assert_ne!(
        code, 0,
        "a main checkout must not reach a lane's admin: {output}"
    );
    let (code, output) = run(
        "printf 'x\\n' > note.txt && git status -s",
        &trunk,
        Some(&sandbox),
    )?;
    assert_eq!(code, 0, "ordinary work still runs there: {output}");
    Ok(())
}

const COMMIT: &str = "GIT_CONFIG_GLOBAL=/dev/null git -c user.email=y@e.com -c user.name=y commit -q --allow-empty -m m";

/// A lane where the host puts one: a slot under `~/.yi/lanes`, its record naming the session, on the
/// session's branch; and a sibling on its own. Returns the trunk, the lane and its profile.
fn held_lane(root: &Path, home: &Path) -> Result<(PathBuf, PathBuf, Sandbox), Box<dyn Error>> {
    let (trunk, _detached) = linked_worktree(root)?;
    let pool = home.join(".yi/lanes/key");
    std::fs::create_dir_all(&pool)?;
    let lane = pool.join("0");
    let text = lane.to_string_lossy();
    git(&trunk, &["worktree", "add", "-q", "-b", "yi/held", &text])?;
    std::fs::write(pool.join("0.json"), r#"{"session":"held"}"#)?;
    let sibling = pool.join("1");
    let text = sibling.to_string_lossy();
    git(&trunk, &["worktree", "add", "-q", "-b", "yi/other", &text])?;
    let sandbox = lane_profile(root, &lane, home)?;
    Ok((trunk, lane, sandbox))
}

/// The work a lane exists for still runs with the common dir taken away, and git says nothing of
/// the `packed-refs` it can no longer write.
#[test]
fn a_lane_commits_on_its_own_branch_and_stashes() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("own-refs")?;
    let (_trunk, lane, sandbox) = held_lane(&root, &home)?;
    let before = git(&lane, &["rev-parse", "HEAD"])?;
    let (code, output) = run(COMMIT, &lane, Some(&sandbox))?;
    assert_eq!(code, 0, "{output}");
    assert!(!output.contains("packed-refs"), "{output}");
    assert_ne!(git(&lane, &["rev-parse", "yi/held"])?, before);
    let stash = "printf 'x\\n' >> README.md && git stash -q && git stash pop -q";
    let (code, output) = run(stash, &lane, Some(&sandbox))?;
    assert_eq!(code, 0, "{output}");
    Ok(())
}

/// Incident: the whole common dir was writable, so a contained process deleted a sibling's ref and
/// moved trunk.
#[test]
fn a_lane_cannot_move_trunk_or_a_siblings_branch() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("other-refs")?;
    let (trunk, lane, sandbox) = held_lane(&root, &home)?;
    let tip = git(&trunk, &["rev-parse", "main"])?;
    let common = trunk.join(".git");
    for attempt in [
        format!(
            "git update-ref refs/heads/main {tip}~0 && git update-ref refs/heads/main $(git commit-tree -m x $(git write-tree))"
        ),
        "git update-ref -d refs/heads/yi/other".to_owned(),
        format!("rm {}", common.join("refs/heads/yi/other").display()),
        format!("printf x >> {}", common.join("packed-refs").display()),
        format!(
            "printf '{tip} refs/heads/forged\\n' > {0}.lock; mv {0}.lock {0}",
            common.join("packed-refs").display()
        ),
    ] {
        run(&attempt, &lane, Some(&sandbox))?;
    }
    assert_eq!(git(&trunk, &["rev-parse", "main"])?, tip);
    assert!(
        git(&trunk, &["rev-parse", "yi/other"]).is_ok(),
        "the sibling's ref went"
    );
    Ok(())
}

/// `HEAD` is contained-writable: a planted one must not grant trunk on the next spawn.
#[test]
fn a_planted_head_grants_nothing_on_the_next_spawn() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("planted-grant")?;
    let (trunk, lane, _sandbox) = held_lane(&root, &home)?;
    let admin = git(&lane, &["rev-parse", "--absolute-git-dir"])?;
    let tip = git(&trunk, &["rev-parse", "main"])?;
    git(&trunk, &["branch", "release"])?;
    for planted in ["main", "yi/other", "release"] {
        std::fs::write(
            Path::new(&admin).join("HEAD"),
            format!("ref: refs/heads/{planted}\n"),
        )?;
        let next = lane_profile(&root, &lane, &home)?;
        let (code, output) = run(COMMIT, &lane, Some(&next))?;
        assert_ne!(code, 0, "{planted}: {output}");
    }
    assert_eq!(git(&trunk, &["rev-parse", "main"])?, tip);
    assert_eq!(git(&trunk, &["rev-parse", "release"])?, tip);
    Ok(())
}

/// Dies with a refused ref naming a path the retry never spells: `retried_by` never matched
/// `git switch -c x`, so the same refusal came back forever and nothing asked.
#[test]
fn a_new_branch_refused_in_a_lane_asks_the_next_time() -> TestResult {
    seatbelt()?;
    let (root, _project, home) = workspace("switch")?;
    let (_trunk, lane, sandbox) = held_lane(&root, &home)?;
    let command = "git switch -q -c feature";
    let (code, output) = run(command, &lane, Some(&sandbox))?;
    assert_ne!(code, 0, "{output}");
    let refusal = sandbox_refusal(&sandbox, &lane, Some(code), &output, command).ok_or(output)?;
    assert!(matches!(refusal, SandboxRefusal::Scopes(_)), "{refusal:?}");
    assert!(refusal.retried_by(&sandbox, &lane, command));
    Ok(())
}

/// A `sandbox-exec` run inside the sandbox cannot apply its profile: the real binary prints
/// `sandbox-exec: sandbox_apply: Operation not permitted` and exits 71. From a cwd the outer
/// profile denies, the errno reader took `<cwd>/sandbox_apply` for a refused write, a path no
/// approval widens. It stays the pathless refusal a nested sandbox always was, which an
/// interactive approval runs outside, and the result says what failed.
#[test]
fn a_nested_sandbox_apply_failure_is_no_refused_write() -> TestResult {
    let (_root, project, _scratch_home) = workspace("nested")?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let outer = Sandbox::for_workspace(&project, &home, None);
    if !outer.denies_write(&home) {
        return Ok(());
    }
    let command = "/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true";
    let (code, output) = run(command, &home, Some(&outer))?;
    assert_eq!(code, 71, "{output}");
    let found = sandbox_refusal(&outer, &home, Some(code), &output, "python3 nest.py");
    assert_eq!(
        found,
        Some(SandboxRefusal::Scopes(vec!["python3".to_owned()])),
        "{output}"
    );
    let mut context = ToolContext::new(home.clone());
    context.sandbox = Some(outer);
    let mut input = serde_json::Map::new();
    input.insert("command".to_owned(), command.into());
    let result = yi_tools::BashTool::default()
        .execute(input, &context)
        .result;
    let text = format!("{:?}", result.content);
    assert!(
        text.contains("could not apply its profile") && text.contains("no refused write"),
        "{text}"
    );
    assert!(
        result.details["sandboxRefusal"]["path"].is_null(),
        "{}",
        result.details
    );
    Ok(())
}

/// Only the line `sandbox-exec` itself prints is the note; a program naming the words, or
/// failing to touch a file of that name, is not.
#[test]
fn only_sandbox_exec_speaking_is_a_nested_apply_failure() {
    for output in [
        "echo: sandbox_apply\n",
        "the sandbox-exec: sandbox_apply: line is quoted\n",
        "touch: sandbox_apply: Operation not permitted\n",
    ] {
        assert_eq!(yi_tools::nested_sandbox_note(output), None, "{output}");
    }
}

/// `ping` to a host past loopback fails in `sendto` with exit 2, which the exit filter skips as a
/// shell failure; the sandbox refused it, and the hint names ICMP rather than a generic network.
#[test]
fn a_refused_ping_gets_its_own_hint() -> TestResult {
    let (_root, project, home) = workspace("ping")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let (code, output) = run("ping -c1 -W1 1.1.1.1", &project, Some(&sandbox))?;
    let refusal = sandbox_refusal(
        &sandbox,
        &project,
        Some(code),
        &output,
        "ping -c1 -W1 1.1.1.1",
    )
    .ok_or_else(|| format!("exit {code}: {output}"))?;
    let hint = denial_hint(&refusal);
    assert!(hint.contains("ICMP") && hint.contains("`ping`"), "{hint}");
    // Another program refused beside it: ICMP is named for none of them.
    let mixed = "ping -c1 -W1 1.1.1.1; curl -sS -m5 https://example.com";
    let (code, output) = run(mixed, &project, Some(&sandbox))?;
    let refusal = sandbox_refusal(&sandbox, &project, Some(code), &output, mixed)
        .ok_or_else(|| format!("exit {code}: {output}"))?;
    assert!(!denial_hint(&refusal).contains("ICMP"), "{refusal:?}");
    let (code, output) = run("ping -c1 127.0.0.1", &project, Some(&sandbox))?;
    assert_eq!(code, 0, "loopback ping works: {output}");
    assert_eq!(
        sandbox_refusal(
            &sandbox,
            &project,
            Some(code),
            &output,
            "ping -c1 127.0.0.1"
        ),
        None
    );
    Ok(())
}

/// A name that fails to resolve behind `|| true` or a pipe exits 0. The output is a note and never
/// a recorded refusal, since fetched or printed text can quote the phrase and no ask may follow
/// it: a `curl -s` of a page holding the phrase, a python print and a grep are the other side.
#[test]
fn a_network_refusal_behind_a_zero_exit_is_a_note_and_never_recorded() -> TestResult {
    let (_root, project, home) = workspace("zero")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let phrase = "curl: (6) Could not resolve host: example.com";
    std::fs::write(
        project.join("page.html"),
        "<p>Could not resolve host: example.com</p>\n",
    )?;
    std::fs::write(project.join("quoted.html"), format!("{phrase}\n"))?;
    let page = format!("curl -s file://{}/page.html", project.display());
    let print = "python3 -c \"print('Could not resolve host: example.com')\"".to_owned();
    let grep = "grep -rn 'resolve host' .".to_owned();
    for command in [
        "curl -sS -m5 https://example.com || echo done",
        "git ls-remote https://github.com/octocat/Hello-World 2>&1 | tail -3",
    ] {
        let (code, output) = run(command, &project, Some(&sandbox))?;
        assert_eq!(code, 0, "{command}: {output}");
        assert!(
            yi_tools::exit_zero_note(&output, command).is_some(),
            "{command}: {output}"
        );
        assert_eq!(
            sandbox_refusal(&sandbox, &project, Some(code), &output, command),
            None
        );
    }
    for command in [page, print, grep] {
        let (code, output) = run(&command, &project, Some(&sandbox))?;
        assert_eq!(code, 0, "{command}: {output}");
        assert!(output.contains("resolve host"), "{command}: {output}");
        assert_eq!(
            yi_tools::exit_zero_note(&output, &command),
            None,
            "{command}"
        );
        assert_eq!(
            sandbox_refusal(&sandbox, &project, Some(code), &output, &command),
            None
        );
    }
    // Through the bash tool: the note shows, the broker is handed nothing to remember.
    let mut context = ToolContext::new(project.clone());
    context.sandbox = Some(sandbox);
    for (command, noted) in [
        (
            "curl -sS -m5 https://example.com || echo done".to_owned(),
            true,
        ),
        (
            format!("curl -s file://{}/page.html", project.display()),
            false,
        ),
    ] {
        let mut input = serde_json::Map::new();
        input.insert("command".to_owned(), command.clone().into());
        let result = yi_tools::BashTool::default()
            .execute(input, &context)
            .result;
        let text = format!("{:?}", result.content);
        assert_eq!(
            text.contains("nothing is recorded"),
            noted,
            "{command}: {text}"
        );
        assert!(
            result.details["sandboxRefusal"].is_null(),
            "{command}: {}",
            result.details
        );
    }
    Ok(())
}

/// What the test binary itself reports; the next test runs it under another sandbox.
#[test]
fn availability_reports_to_stdout() {
    println!("available={}", Sandbox::available());
}

fn reported_availability(outer: Option<&str>) -> Result<String, Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let args = [
        "--exact",
        "sandbox::availability_reports_to_stdout",
        "--nocapture",
    ];
    let mut command = match outer {
        Some(profile) => {
            let mut command = yi_tools::command(yi_tools::sandbox::SEATBELT);
            command.args(["-p", profile]).arg(&exe);
            command
        }
        None => yi_tools::command(&exe),
    };
    let out = command.args(args).output()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("available=").map(str::to_owned))
        .ok_or_else(|| format!("no report: {}", String::from_utf8_lossy(&out.stderr)).into())
}

/// Incident: a process an outer sandbox contains read as sandbox-capable, so its bash and kernel
/// died on `sandbox_apply: Operation not permitted`.
#[test]
fn a_process_inside_a_sandbox_is_not_sandbox_capable() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let outer = "(version 1)(allow default)(deny file-write* (subpath \"/yi-no-such-root\"))";
    assert_eq!(reported_availability(None)?, "true");
    assert_eq!(reported_availability(Some(outer))?, "false");
    Ok(())
}

/// A contained install writes the package stores; what the host's next build runs stays out of reach.
#[test]
fn a_contained_install_writes_the_caches_and_not_what_cargo_runs() -> TestResult {
    seatbelt()?;
    let (root, project, home) = workspace("caches")?;
    let fake_home = root.join("home");
    std::fs::create_dir_all(fake_home.join(".cargo/registry"))?;
    std::fs::create_dir_all(fake_home.join(".cargo/bin"))?;
    let mut sandbox = Sandbox::for_workspace(&project, &home, None);
    sandbox.writable = vec![project.clone()];
    let sandbox = sandbox.with_caches_at(&fake_home, None, None);
    let write = |target: PathBuf| -> Result<i32, Box<dyn Error>> {
        let command = format!("printf x >> {}", target.display());
        Ok(run(&command, &project, Some(&sandbox))?.0)
    };
    assert_eq!(write(fake_home.join(".cargo/registry/x"))?, 0);
    assert_ne!(write(fake_home.join(".cargo/bin/cargo"))?, 0);
    assert_ne!(write(fake_home.join(".cargo/config.toml"))?, 0);
    Ok(())
}

/// Incident shape: `CARGO_HOME=$HOME` is how one setup keeps cargo's dirs at the top of home.
#[test]
fn a_store_at_home_or_above_it_adds_no_root() -> TestResult {
    let (root, project, home) = workspace("caches-home")?;
    let fake_home = root.join("home");
    std::fs::create_dir_all(&fake_home)?;
    let base = Sandbox::for_workspace(&project, &home, None);
    let widened = |cargo: Option<PathBuf>, uv: Option<PathBuf>| {
        let with = base.clone().with_caches_at(&fake_home, cargo, uv);
        (with.writable.into_iter())
            .filter(|dir| !base.writable.contains(dir))
            .collect::<Vec<PathBuf>>()
    };
    let cargo_at_home = widened(Some(fake_home.clone()), None);
    assert!(
        !cargo_at_home.contains(&fake_home.join("registry")),
        "{cargo_at_home:?}"
    );
    assert!(
        !cargo_at_home.contains(&fake_home.join(".package-cache")),
        "{cargo_at_home:?}"
    );
    let uv_at_home = widened(None, Some(fake_home.clone()));
    assert!(!uv_at_home.contains(&fake_home), "{uv_at_home:?}");
    let above = root.to_path_buf();
    let uv_above = widened(None, Some(above.clone()));
    assert!(!uv_above.contains(&above), "{uv_above:?}");
    Ok(())
}

fn spawn_with(env: Vec<(String, String)>, rows: Vec<String>) -> yi_tools::SpawnFn {
    Arc::new(move |_command: &str| {
        let rows = rows.clone();
        Ok(yi_tools::Spawn {
            env: env.clone(),
            assign: Vec::new(),
            after: Box::new(move || rows),
        })
    })
}

/// `ps` reads `argv` for every user on the box, so a proxy token rides only the environment.
#[test]
fn what_a_spawn_is_handed_reaches_its_child_and_never_its_argv() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("spawn-env")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let pidfile = project.join("pid");
    let spawn = spawn_with(
        vec![("PROBE_SECRET".to_owned(), "t0ken".to_owned())],
        Vec::new(),
    );
    let command = format!(
        "printf %s \"$PROBE_SECRET:$$\" > {}; sleep 30; true",
        pidfile.display()
    );
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let job = yi_tools::jobs::spawn_job(&command, &project, &never, Some(&sandbox), Some(&spawn))?;
    let written = (0..500)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::fs::read_to_string(&pidfile)
                .ok()
                .filter(|text| !text.is_empty())
        })
        .ok_or("the child never wrote")?;
    let (secret, pid) = written.split_once(':').ok_or("no pid")?;
    assert_eq!(secret, "t0ken");
    let args = yi_tools::keyless_command("ps")
        .args(["-ww", "-o", "command=", "-p", pid])
        .output()?;
    let args = String::from_utf8_lossy(&args.stdout);
    let _ = yi_tools::jobs::registry().kill(job);
    assert!(
        args.contains("sleep 30"),
        "ps read the wrong process: {args}"
    );
    assert!(!args.contains("t0ken"), "{args}");
    Ok(())
}

#[test]
fn a_refused_spawn_never_starts_and_says_why() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("spawn-refused")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let marker = project.join("ran");
    let refused: yi_tools::SpawnFn = Arc::new(|_command: &str| {
        Err(yi_tools::SpawnRefused::Egress {
            why: "the egress proxy cannot listen".to_owned(),
        })
    });
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let command = format!("touch {}", marker.display());
    let why = yi_tools::jobs::spawn_job(&command, &project, &never, Some(&sandbox), Some(&refused))
        .err()
        .ok_or("the spawn started")?;
    assert!(why.contains("the egress proxy cannot listen"), "{why}");
    assert!(!marker.exists());
    Ok(())
}

/// A row the reducer could cut is a refusal the model never reads.
#[test]
fn what_the_host_says_after_a_spawn_reaches_the_model() -> TestResult {
    seatbelt()?;
    let (_root, project, home) = workspace("spawn-rows")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let mut context = yi_tools::ToolContext::new(project.clone());
    context.spawn = Some(spawn_with(
        Vec::new(),
        vec!["[egress: refused a.example: test]".to_owned()],
    ));
    let timeout = std::time::Duration::from_secs(30);
    let run =
        yi_tools::run_or_background("printf ok", &context, None, timeout, Some(&sandbox), None)?;
    let yi_tools::Run::Finished(capture) = run else {
        return Err("the command did not finish".into());
    };
    assert_eq!(
        capture.rows,
        vec!["[egress: refused a.example: test]".to_owned()]
    );
    Ok(())
}

/// The transport needle was a bare `' not allowed`, so a tool refusing an argument read as a
/// sandbox network denial and sent the person to an approval that could not help.
#[test]
fn a_tools_own_not_allowed_is_not_a_network_denial() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let found = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(1),
        "error: hook 'pre-lint' not allowed\n",
        "lint",
    );
    assert_eq!(found, None);
    let git = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(128),
        "fatal: transport 'file' not allowed\n",
        "git fetch origin",
    );
    assert!(matches!(git, Some(SandboxRefusal::Scopes(_))), "{git:?}");
    Ok(())
}

/// Dies with the generic `' not allowed` needle: a compiler echoing a phrase must not read as
/// git's transport refusal, and git's own refusal must still classify and ask.
#[test]
fn only_gits_transport_refusal_is_a_network_denial() -> TestResult {
    let (sandbox, _home) = home_project()?;
    let transport = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(128),
        "fatal: transport 'https' not allowed\n",
        "git fetch origin",
    );
    assert!(transport.is_some(), "git's transport refusal must classify");
    let unrelated = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(1),
        "ValueError: 'None' not allowed as a mode\n",
        "python3 check.py",
    );
    assert_eq!(unrelated, None, "{unrelated:?}");
    let merely_named = sandbox_refusal(
        &sandbox,
        Path::new("/work"),
        Some(1),
        "warning: no rule for the transport 'bluetooth' in this schema\n",
        "python3 check.py",
    );
    assert_eq!(merely_named, None, "{merely_named:?}");
    Ok(())
}
