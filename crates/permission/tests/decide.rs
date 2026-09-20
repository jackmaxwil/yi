use std::error::Error;
use std::path::PathBuf;

use yi_permission::{
    CatastrophicContext, ConfigRule, ConfigRuleAction, Decision, Hold, HoldSource, ParseOutcome,
    PermissionMode, SessionRules, ToolCall, canonical_command_identity, canonical_tool_identity,
    command_reads_credentials, decide, git_dirs, grants, is_catastrophic, lexical_normalize,
    parse_command,
};
use yi_types::permission::{RuleDecision, RuleKind, SessionPermissionState};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn context() -> CatastrophicContext {
    CatastrophicContext {
        home_dir: Some(PathBuf::from("/home/user")),
        working_dir: Some(PathBuf::from("/home/user/project")),
        workspace_git: vec![PathBuf::from("/home/user/project/.git")],
    }
}

fn bash_call<'a>(command: &'a str, canonical: &'a str) -> ToolCall<'a> {
    ToolCall {
        tool_name: "bash",
        reads_only: false,
        irreversible: true,
        in_workspace: false,
        rule_kind: RuleKind::Command,
        canonical,
        display: command,
        targets: &[],
        command: Some(command),
    }
}

fn read_call<'a>(targets: &'a [PathBuf]) -> ToolCall<'a> {
    ToolCall {
        tool_name: "read",
        reads_only: true,
        irreversible: false,
        in_workspace: true,
        rule_kind: RuleKind::StructuredTool,
        canonical: "read-canonical",
        display: "read file",
        targets,
        command: None,
    }
}

#[test]
fn lexical_normalizer_pops_dotdot_without_touching_the_filesystem() -> TestResult {
    assert_eq!(
        lexical_normalize(&PathBuf::from("/home/user/../..")),
        PathBuf::from("/")
    );
    assert_eq!(
        lexical_normalize(&PathBuf::from("/a/./b/../c")),
        PathBuf::from("/a/c")
    );
    Ok(())
}

#[test]
fn catastrophic_paths_cover_system_home_credentials_and_workspace_git() -> TestResult {
    let context = context();
    for path in [
        "/",
        "/etc",
        "/etc/passwd",
        "/usr/bin/thing",
        "/home/user",
        "/home/user/.ssh/id_rsa",
        "/home/user/.config",
        "/home/user/project/.git",
        "/home/user/project/.git/HEAD",
    ] {
        assert!(is_catastrophic(&PathBuf::from(path), &context), "{path}");
    }
    for path in [
        "/home/user/project/src/main.rs",
        "/home/user/.config/app/settings.json",
        "/home/user/notes.txt",
    ] {
        assert!(!is_catastrophic(&PathBuf::from(path), &context), "{path}");
    }
    Ok(())
}

#[test]
fn catastrophic_command_targets_are_denied_even_in_yolo() -> TestResult {
    let context = context();
    let session = SessionRules::new();
    let call = bash_call("rm -rf ~/.ssh", "c1");
    let decision = decide(&call, PermissionMode::Yolo, &[], &session, &[], &context);
    let Decision::Deny { reason } = decision else {
        return Err("expected deny".into());
    };
    assert!(reason.contains("protected path"), "{reason}");
    assert!(reason.contains("denied in every mode"), "{reason}");

    let traversal = bash_call("rm -rf ~/project/../../", "c2");
    assert!(matches!(
        decide(
            &traversal,
            PermissionMode::Yolo,
            &[],
            &session,
            &[],
            &context
        ),
        Decision::Deny { .. }
    ));
    Ok(())
}

#[test]
fn configured_deny_beats_session_allow() -> TestResult {
    let context = context();
    let mut session = SessionRules::new();
    let canonical = canonical_command_identity("cargo publish", "/home/user/project");
    session.insert(
        RuleKind::Command,
        &canonical,
        "cargo publish",
        RuleDecision::Allow,
    )?;
    let config = vec![
        ConfigRule::new("bash", "*publish*", ConfigRuleAction::Deny).map_err(|e| e.to_string())?,
    ];

    let call = bash_call("cargo publish", &canonical);
    let decision = decide(
        &call,
        PermissionMode::Yolo,
        &config,
        &session,
        &[],
        &context,
    );
    assert!(matches!(decision, Decision::Deny { .. }), "{decision:?}");
    Ok(())
}

#[test]
fn session_allow_rule_admits_the_exact_call() -> TestResult {
    let context = context();
    let mut session = SessionRules::new();
    let canonical = canonical_command_identity("cargo build", "/home/user/project");
    session.insert(
        RuleKind::Command,
        &canonical,
        "cargo build",
        RuleDecision::Allow,
    )?;

    let call = bash_call("cargo build", &canonical);
    let allowed = decide(&call, PermissionMode::Ask, &[], &session, &[], &context);
    assert_eq!(
        allowed,
        Decision::Allow {
            reason: "allowed by session rule".to_owned()
        }
    );

    let other_canonical = canonical_command_identity("cargo test", "/home/user/project");
    let other = bash_call("cargo test", &other_canonical);
    assert!(matches!(
        decide(&other, PermissionMode::Ask, &[], &session, &[], &context),
        Decision::Ask { .. }
    ));
    Ok(())
}

#[test]
fn holds_turn_matching_calls_into_ask_with_reason() -> TestResult {
    let context = context();
    let session = SessionRules::new();
    let holds = vec![Hold {
        pattern: "migrations".to_owned(),
        reason: "schema migrations need review this session".to_owned(),
        source: HoldSource::User,
        expires_at_ms: None,
    }];
    let call = bash_call("rm migrations/0001.sql", "c3");
    let Decision::Ask { description, .. } =
        decide(&call, PermissionMode::Yolo, &[], &session, &holds, &context)
    else {
        return Err("expected ask".into());
    };
    assert!(
        description.contains("schema migrations need review"),
        "{description}"
    );
    Ok(())
}

#[test]
fn mode_fallbacks_match_the_reference_gate() -> TestResult {
    let context = context();
    let session = SessionRules::new();
    let targets = vec![PathBuf::from("/home/user/project/src/main.rs")];
    let read = read_call(&targets);
    assert_eq!(
        decide(&read, PermissionMode::Ask, &[], &session, &[], &context),
        Decision::Allow {
            reason: "read-only invocation allowed".to_owned()
        }
    );
    let write = bash_call("cargo build", "c4");
    assert!(matches!(
        decide(&write, PermissionMode::Ask, &[], &session, &[], &context),
        Decision::Ask { .. }
    ));
    assert_eq!(
        decide(&write, PermissionMode::Yolo, &[], &session, &[], &context),
        Decision::Allow {
            reason: "allowed by yolo mode".to_owned()
        }
    );
    Ok(())
}

#[test]
fn unparseable_commands_are_their_own_outcome() -> TestResult {
    assert_eq!(
        parse_command("cargo build"),
        ParseOutcome::Parsed(vec!["cargo build".to_owned()])
    );
    assert_eq!(parse_command("rg foo | head"), ParseOutcome::Unparsed);
    assert_eq!(parse_command("a && b"), ParseOutcome::Unparsed);
    assert_eq!(parse_command("echo $(whoami)"), ParseOutcome::Unparsed);
    Ok(())
}

#[test]
fn session_state_round_trips_and_rejects_tampering() -> TestResult {
    let mut session = SessionRules::new();
    let canonical = canonical_command_identity("just check", "/home/user/project");
    session.insert(
        RuleKind::Command,
        &canonical,
        "just check",
        RuleDecision::Allow,
    )?;
    let state = session.state().clone();

    let reloaded = SessionRules::load(state.clone())?;
    assert_eq!(
        reloaded.decision_for(RuleKind::Command, &canonical),
        Some(RuleDecision::Allow)
    );

    let mut tampered = state;
    if let Some(rule) = tampered.rules.first_mut() {
        rule.generation = 0;
    }
    assert!(SessionRules::load(tampered).is_err());

    let bad_version = SessionPermissionState {
        version: 1,
        ..SessionPermissionState::default()
    };
    assert!(SessionRules::load(bad_version).is_err());
    Ok(())
}

fn auto(call: &ToolCall<'_>) -> Decision {
    decide(
        call,
        PermissionMode::Auto,
        &[],
        &SessionRules::new(),
        &[],
        &context(),
    )
}

fn write_call<'a>(targets: &'a [PathBuf], in_workspace: bool) -> ToolCall<'a> {
    ToolCall {
        tool_name: "write",
        reads_only: false,
        irreversible: true,
        in_workspace,
        rule_kind: RuleKind::StructuredTool,
        canonical: "write-canonical",
        display: "write file",
        targets,
        command: None,
    }
}

/// Incident: `rm -rf /` and `rm -rf .git` both walked past the denylist, the
/// first because trimming the trailing slash left an empty path that expanded
/// to the working directory, the second because a token without a slash was
/// never treated as a path at all. Yolo is the mode that made it visible: it
/// has nothing else between the model and the command.
#[test]
fn the_denylist_reads_a_bare_root_and_a_bare_dotfile() -> TestResult {
    for command in [
        "rm -rf /",
        "rm -rf /*",
        "rm -rf .git",
        "rm -rf .git/objects",
        "rm -rf /etc",
        "sudo rm -rf /usr",
    ] {
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let call = bash_call(command, "canonical");
            let decision = decide(&call, mode, &[], &SessionRules::new(), &[], &context());
            assert!(
                matches!(decision, Decision::Deny { .. }),
                "{command:?} in {mode:?} must be denied, got {decision:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn a_credential_read_is_named_before_it_happens() -> TestResult {
    let context = context();
    assert!(command_reads_credentials("cat /home/user/.ssh/id_rsa", &context).is_some());
    assert!(command_reads_credentials("rg secret ~/.aws/credentials", &context).is_some());
    assert!(command_reads_credentials("cat $HOME/.gnupg/secring.gpg", &context).is_some());
    assert!(command_reads_credentials("cat /etc/hosts", &context).is_none());
    assert!(command_reads_credentials("cat notes.md", &context).is_none());
    // The flag is not a path, and the verb is not an argument.
    assert!(command_reads_credentials("ssh -i x host", &context).is_none());

    let call = bash_call("cat ~/.ssh/id_rsa", "canonical");
    match auto(&call) {
        Decision::Ask { description, .. } => {
            assert!(description.contains("credential store"), "{description}")
        }
        other => return Err(format!("expected an ask, got {other:?}").into()),
    }
    Ok(())
}

/// Incident: under `--yolo` a `read` of /etc/nginx/nginx.conf was refused as catastrophic
/// while `bash cat` printed the same bytes (the 2026-09-10 harness audit, S3). A read is
/// refused for what it leaks or never finishes: a key, the workspace .git, a directory a walk
/// would carry into a key store, a device.
#[test]
fn a_read_of_a_system_path_is_allowed_but_a_key_is_not() -> TestResult {
    let session = SessionRules::new();
    for mode in [
        PermissionMode::Ask,
        PermissionMode::Auto,
        PermissionMode::Yolo,
    ] {
        let read = |path: &str| {
            let targets = [PathBuf::from(path)];
            decide(&read_call(&targets), mode, &[], &session, &[], &context())
        };
        for path in [
            "/etc/nginx/nginx.conf",
            "/usr/include/stdio.h",
            "/proc/self/environ",
        ] {
            let decision = read(path);
            assert!(
                matches!(decision, Decision::Allow { .. }),
                "{path} in {mode:?}: {decision:?}"
            );
        }
        for path in [
            "/home/user/.ssh/id_rsa",
            "/home/user/project/.git/config",
            "/home/user",
            "/",
            "/dev/zero",
            "/proc/self/root/home/user/.ssh/id_rsa",
            "/proc/self/cwd/.git/config",
        ] {
            let decision = read(path);
            assert!(
                matches!(decision, Decision::Deny { .. }),
                "{path} in {mode:?}: {decision:?}"
            );
        }
    }
    // Other users' key stores sit under /home and /Users whatever this HOME is.
    let root = CatastrophicContext {
        home_dir: Some(PathBuf::from("/root")),
        ..context()
    };
    for path in ["/home", "/Users"] {
        let targets = [PathBuf::from(path)];
        let decision = decide(
            &read_call(&targets),
            PermissionMode::Yolo,
            &[],
            &session,
            &[],
            &root,
        );
        assert!(
            matches!(decision, Decision::Deny { .. }),
            "{path}: {decision:?}"
        );
    }
    // A write, and a read-kind call that reports itself irreversible (grep's replace+apply),
    // still meet the whole denylist.
    let targets = [PathBuf::from("/etc/nginx/nginx.conf")];
    let mut rewrite = read_call(&targets);
    rewrite.irreversible = true;
    for call in [rewrite, write_call(&targets, false)] {
        let decision = decide(&call, PermissionMode::Yolo, &[], &session, &[], &context());
        assert!(matches!(decision, Decision::Deny { .. }), "{decision:?}");
    }
    Ok(())
}

/// Incident: a `read` of `~/.ssh/id_rsa` reached the check as a relative path and passed it;
/// only the read's own failure kept the key out of the transcript.
#[test]
fn a_tilde_read_is_judged_under_home() -> TestResult {
    let session = SessionRules::new();
    for mode in [
        PermissionMode::Ask,
        PermissionMode::Auto,
        PermissionMode::Yolo,
    ] {
        let read = |path: &str| {
            let targets = [PathBuf::from(path)];
            decide(&read_call(&targets), mode, &[], &session, &[], &context())
        };
        let key = read("~/.ssh/id_rsa");
        match &key {
            Decision::Deny { reason } => {
                assert!(reason.contains("/home/user/.ssh/id_rsa"), "{reason}")
            }
            other => return Err(format!("{mode:?}: expected a deny, got {other:?}").into()),
        }
        let notes = read("~/notes.txt");
        assert!(
            matches!(notes, Decision::Allow { .. }),
            "{mode:?}: {notes:?}"
        );
    }
    Ok(())
}

/// Auto's own arm: a write the turn checkpoint can undo runs, a write outside
/// the tree asks, and a tool that reports itself reversible runs.
#[test]
fn auto_allows_what_a_checkpoint_can_undo() -> TestResult {
    let inside = vec![PathBuf::from("/home/user/project/src/lib.rs")];
    assert!(matches!(
        auto(&write_call(&inside, true)),
        Decision::Allow { .. }
    ));
    let outside = vec![PathBuf::from("/home/user/elsewhere/notes.md")];
    assert!(matches!(
        auto(&write_call(&outside, false)),
        Decision::Ask { .. }
    ));

    let mut reversible = write_call(&[], false);
    reversible.irreversible = false;
    reversible.tool_name = "ipython";
    let decision = auto(&reversible);
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "a tool that screens its own call keeps that answer: {decision:?}"
    );
    let mut opaque = write_call(&[], false);
    opaque.tool_name = "ipython";
    assert!(matches!(auto(&opaque), Decision::Ask { .. }));
    Ok(())
}

#[test]
fn auto_reads_a_command_segment_by_segment() -> TestResult {
    let allowed = bash_call("cargo test && git status", "canonical");
    assert!(matches!(auto(&allowed), Decision::Allow { .. }));

    let mixed = bash_call("cargo test && rm -rf target", "canonical");
    match auto(&mixed) {
        Decision::Ask { description, .. } => {
            assert!(description.contains("destructive"), "{description}");
        }
        other => return Err(format!("expected an ask, got {other:?}").into()),
    }

    // Unreadable is contained, not refused: the broker turns that into a
    // question only where no sandbox can enforce it.
    let unreadable = bash_call("cargo test > log.txt", "canonical");
    assert!(matches!(auto(&unreadable), Decision::Contain { .. }));
    let unknown = bash_call("just check", "canonical");
    assert!(matches!(auto(&unknown), Decision::Contain { .. }));
    Ok(())
}

/// A trunk checkout with one linked worktree, laid out by hand: `git worktree add` writes
/// exactly these files, and the reader must follow them without spawning git.
fn linked_layout(
    root: &std::path::Path,
    relative: bool,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let common = root.join("trunk/.git");
    let gitdir = common.join("worktrees/wt");
    std::fs::create_dir_all(&gitdir)?;
    std::fs::write(common.join("HEAD"), "ref: refs/heads/main\n")?;
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature\n")?;
    std::fs::write(gitdir.join("commondir"), "../..\n")?;
    let tree = root.join("wt");
    std::fs::create_dir_all(tree.join("src"))?;
    let pointer = if relative {
        "gitdir: ../trunk/.git/worktrees/wt\n".to_owned()
    } else {
        format!("gitdir: {}\n", gitdir.display())
    };
    std::fs::write(tree.join(".git"), pointer)?;
    Ok((tree, common))
}

#[test]
fn git_dirs_follow_a_worktree_pointer_to_its_common_dir() -> TestResult {
    for relative in [false, true] {
        let root = Scratch::new("yi-permission-gitdirs")?;
        let (tree, common) = linked_layout(&root, relative)?;
        let gitdir = common.join("worktrees/wt");
        let found = git_dirs(&tree.join("src"));
        assert_eq!(
            found,
            vec![lexical_normalize(&gitdir), lexical_normalize(&common)],
            "relative pointer: {relative}"
        );
        assert_eq!(
            git_dirs(&root.join("trunk")),
            vec![lexical_normalize(&common)],
            "a primary checkout's git dir is its own common dir"
        );
    }
    let bare = Scratch::new("yi-permission-gitdirs-none")?;
    assert!(git_dirs(&bare).is_empty(), "no checkout, no git dirs");
    Ok(())
}

#[test]
fn rm_rf_of_a_linked_worktrees_common_dir_is_denied_in_every_mode() -> TestResult {
    let root = Scratch::new("yi-permission-common-rm")?;
    let (tree, common) = linked_layout(&root, false)?;
    let context = CatastrophicContext::detect(&tree);
    let session = SessionRules::new();
    for target in [
        common.clone(),
        common.join("worktrees/wt"),
        common.join("objects"),
    ] {
        let command = format!("rm -rf {}", target.display());
        let call = bash_call(&command, "canonical");
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let decision = decide(&call, mode, &[], &session, &[], &context);
            assert!(
                matches!(decision, Decision::Deny { .. }),
                "{command} in {mode:?}: {decision:?}"
            );
        }
    }
    Ok(())
}

/// The canonical carries the whole arguments JSON in the broker, patch and all, so two edits
/// of different files are two identities — as they are here.
fn edit_canonical(targets: &[PathBuf]) -> String {
    canonical_tool_identity(
        "edit",
        &targets
            .iter()
            .map(|target| target.display().to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn edit_call<'a>(targets: &'a [PathBuf], in_workspace: bool, canonical: &'a str) -> ToolCall<'a> {
    ToolCall {
        tool_name: "edit",
        reads_only: false,
        irreversible: true,
        in_workspace,
        rule_kind: RuleKind::StructuredTool,
        canonical,
        display: "edit file",
        targets,
        command: None,
    }
}

fn ask_mode(call: &ToolCall<'_>, session: &SessionRules) -> Decision {
    decide(call, PermissionMode::Ask, &[], session, &[], &context())
}

fn grant(session: &mut SessionRules, call: &ToolCall<'_>, index: usize) -> TestResult {
    let offered = grants(call, &context());
    let chosen = offered.get(index).ok_or("no such grant")?;
    session.insert(
        chosen.kind,
        &chosen.canonical,
        &chosen.label,
        RuleDecision::Allow,
    )?;
    Ok(())
}

#[test]
fn grants_offer_the_target_dir_then_the_tree_root() -> TestResult {
    let targets = [PathBuf::from("/home/user/project/crates/tui/src/app.rs")];
    let labels: Vec<String> = grants(
        &edit_call(&targets, true, &edit_canonical(&targets)),
        &context(),
    )
    .into_iter()
    .map(|grant| grant.label)
    .collect();
    assert_eq!(
        labels,
        ["edits under crates/tui/src", "edits anywhere in this tree"]
    );
    let command = "git worktree add ../a b";
    let canonical = canonical_command_identity(command, "/home/user/project");
    let labels: Vec<String> = grants(&bash_call(command, &canonical), &context())
        .into_iter()
        .map(|grant| grant.label)
        .collect();
    assert_eq!(labels, ["`git worktree` in this tree"]);
    let canonical = canonical_command_identity("rm -rf build", "/home/user/project");
    let labels: Vec<String> = grants(&bash_call("rm -rf build", &canonical), &context())
        .into_iter()
        .map(|grant| grant.label)
        .collect();
    assert_eq!(labels, ["this exact command"]);
    Ok(())
}

#[test]
fn a_directory_grant_allows_later_edits_beneath_it_only() -> TestResult {
    let mut session = SessionRules::new();
    let first = [PathBuf::from("/home/user/project/crates/tui/src/app.rs")];
    grant(
        &mut session,
        &edit_call(&first, true, &edit_canonical(&first)),
        0,
    )?;
    let sibling = [PathBuf::from("/home/user/project/crates/tui/src/render.rs")];
    let nested = [PathBuf::from(
        "/home/user/project/crates/tui/src/app/port.rs",
    )];
    for targets in [&sibling, &nested] {
        let decision = ask_mode(
            &edit_call(targets, true, &edit_canonical(targets)),
            &session,
        );
        assert!(
            matches!(decision, Decision::Allow { .. }),
            "{targets:?}: {decision:?}"
        );
    }
    let above = [PathBuf::from("/home/user/project/crates/tui/Cargo.toml")];
    let decision = ask_mode(&edit_call(&above, true, &edit_canonical(&above)), &session);
    assert!(matches!(decision, Decision::Ask { .. }), "{decision:?}");
    Ok(())
}

#[test]
fn a_tree_grant_never_reaches_outside_the_workspace() -> TestResult {
    let mut session = SessionRules::new();
    let first = [PathBuf::from("/home/user/project/src/main.rs")];
    grant(
        &mut session,
        &edit_call(&first, true, &edit_canonical(&first)),
        1,
    )?;
    let inside = [PathBuf::from("/home/user/project/docs/notes.md")];
    assert!(matches!(
        ask_mode(
            &edit_call(&inside, true, &edit_canonical(&inside)),
            &session
        ),
        Decision::Allow { .. }
    ));
    let outside = [PathBuf::from("/home/user/other/src/main.rs")];
    let decision = ask_mode(
        &edit_call(&outside, false, &edit_canonical(&outside)),
        &session,
    );
    assert!(matches!(decision, Decision::Ask { .. }), "{decision:?}");
    Ok(())
}

#[test]
fn a_scope_grant_allows_the_same_subcommand_with_other_arguments() -> TestResult {
    let mut session = SessionRules::new();
    let cwd = "/home/user/project";
    let first = "git worktree add ../a b";
    let canonical = canonical_command_identity(first, cwd);
    grant(&mut session, &bash_call(first, &canonical), 0)?;
    let again = "git worktree add -b other ../c origin/main";
    let canonical = canonical_command_identity(again, cwd);
    let decision = ask_mode(&bash_call(again, &canonical), &session);
    assert!(matches!(decision, Decision::Allow { .. }), "{decision:?}");
    let other = "git commit -m x";
    let canonical = canonical_command_identity(other, cwd);
    let decision = ask_mode(&bash_call(other, &canonical), &session);
    assert!(matches!(decision, Decision::Ask { .. }), "{decision:?}");
    Ok(())
}

#[test]
fn a_scope_grant_never_covers_destructive_unparsed_or_env_prefixed_commands() -> TestResult {
    let mut session = SessionRules::new();
    let cwd = "/home/user/project";
    let first = "git worktree add ../a b";
    let canonical = canonical_command_identity(first, cwd);
    grant(&mut session, &bash_call(first, &canonical), 0)?;
    for command in [
        "git worktree add ../c d && rm -rf build",
        "git worktree add $(pwd)/c d",
        "GIT_DIR=/elsewhere git worktree add ../c d",
        "git worktree add ../c d > log.txt",
    ] {
        let canonical = canonical_command_identity(command, cwd);
        let decision = ask_mode(&bash_call(command, &canonical), &session);
        assert!(
            !matches!(decision, Decision::Allow { .. }),
            "{command}: {decision:?}"
        );
    }
    Ok(())
}

#[test]
fn catastrophic_and_configured_deny_still_beat_a_grant() -> TestResult {
    let mut session = SessionRules::new();
    let first = [PathBuf::from("/home/user/project/src/main.rs")];
    grant(
        &mut session,
        &edit_call(&first, true, &edit_canonical(&first)),
        1,
    )?;
    let git = [PathBuf::from("/home/user/project/.git/config")];
    let decision = ask_mode(&edit_call(&git, true, &edit_canonical(&git)), &session);
    assert!(matches!(decision, Decision::Deny { .. }), "{decision:?}");

    let cwd = "/home/user/project";
    let command = "git worktree add ../a b";
    let canonical = canonical_command_identity(command, cwd);
    grant(&mut session, &bash_call(command, &canonical), 0)?;
    let config = vec![
        ConfigRule::new("bash", "*worktree*", ConfigRuleAction::Deny).map_err(|e| e.to_string())?,
    ];
    let decision = decide(
        &bash_call(command, &canonical),
        PermissionMode::Ask,
        &config,
        &session,
        &[],
        &context(),
    );
    assert!(matches!(decision, Decision::Deny { .. }), "{decision:?}");
    Ok(())
}

/// A grant's label says "in this tree"; the rule has to mean it. A program named by path, a
/// wrapper, or a later `-C` out of the tree are all ways the promise was broken (D207).
#[test]
fn a_scope_grant_keeps_the_promise_its_label_makes() -> TestResult {
    let mut session = SessionRules::new();
    let cwd = "/home/user/project";
    let first = "git worktree add ../a b";
    let canonical = canonical_command_identity(first, cwd);
    grant(&mut session, &bash_call(first, &canonical), 0)?;
    for command in [
        "git -C /home/user/other-repo worktree remove --force /home/user/precious",
        "git --git-dir=/home/user/other-repo/.git worktree remove ../x",
        "git -C ../.. worktree remove --force x",
    ] {
        let canonical = canonical_command_identity(command, cwd);
        let decision = ask_mode(&bash_call(command, &canonical), &session);
        assert!(
            !matches!(decision, Decision::Allow { .. }),
            "a grant may not follow a command out of the tree: {command}: {decision:?}"
        );
    }

    // A program named by path is a file in the tree, which the model can write; the grant is
    // for the program on PATH, never for `./python3`.
    let mut session = SessionRules::new();
    let first = "python3 scripts/build.py";
    let canonical = canonical_command_identity(first, cwd);
    grant(&mut session, &bash_call(first, &canonical), 0)?;
    for command in ["./python3 evil.py", "/tmp/evil/python3 evil.py"] {
        let canonical = canonical_command_identity(command, cwd);
        let decision = ask_mode(&bash_call(command, &canonical), &session);
        assert!(
            !matches!(decision, Decision::Allow { .. }),
            "{command}: {decision:?}"
        );
    }

    // A wrapper is never the grant: `timeout` would otherwise cover everything it can run.
    let labels: Vec<String> = grants(
        &bash_call(
            "timeout 600 make test",
            &canonical_command_identity("timeout 600 make test", cwd),
        ),
        &context(),
    )
    .into_iter()
    .map(|grant| grant.label)
    .collect();
    assert_eq!(
        labels,
        ["`make test` in this tree"],
        "the wrapper is not the scope"
    );
    Ok(())
}

/// A file at the tree root offered only the tree, so `a` — the narrowest choice a surface
/// shows — silently granted the whole repository (D207).
#[test]
fn a_root_level_edit_offers_the_exact_call_before_the_tree() -> TestResult {
    let targets = [PathBuf::from("/home/user/project/README.md")];
    let labels: Vec<String> = grants(
        &edit_call(&targets, true, &edit_canonical(&targets)),
        &context(),
    )
    .into_iter()
    .map(|grant| grant.label)
    .collect();
    assert_eq!(labels, ["this exact call", "edits anywhere in this tree"]);

    let mut session = SessionRules::new();
    grant(
        &mut session,
        &edit_call(&targets, true, &edit_canonical(&targets)),
        0,
    )?;
    let elsewhere = [PathBuf::from("/home/user/project/.github/workflows/ci.yml")];
    let decision = ask_mode(
        &edit_call(&elsewhere, true, &edit_canonical(&elsewhere)),
        &session,
    );
    assert!(
        matches!(decision, Decision::Ask { .. }),
        "the narrowest grant on a root-level file is that call, not the repository: {decision:?}"
    );
    Ok(())
}
/// A pointer is repository data, and repository data is not a grant: a `.git` file that names
/// `/` would otherwise make the whole filesystem a sandbox writable root.
#[test]
fn git_dirs_refuse_a_pointer_that_is_not_a_git_dir() -> TestResult {
    let root = Scratch::new("yi-permission-pointer")?;
    let (tree, common) = linked_layout(&root, false)?;
    let gitdir = common.join("worktrees/wt");
    for pointer in [
        "gitdir: /\n".to_owned(),
        "gitdir: /etc\n".to_owned(),
        "gitdir: ../../../../../../..\n".to_owned(),
        format!("gitdir: {}\n", root.join("not-a-git-dir").display()),
    ] {
        std::fs::write(tree.join(".git"), &pointer)?;
        assert_eq!(git_dirs(&tree), Vec::<PathBuf>::new(), "{pointer:?}");
    }
    std::fs::write(tree.join(".git"), format!("gitdir: {}\n", gitdir.display()))?;
    for common_text in ["/\n", "../../../../../..\n", "/etc\n"] {
        std::fs::write(gitdir.join("commondir"), common_text)?;
        assert_eq!(
            git_dirs(&tree),
            vec![lexical_normalize(&gitdir)],
            "a commondir that names no git dir leaves the worktree's own: {common_text:?}"
        );
    }
    Ok(())
}
/// A wrapper is not a disguise: the belt reads what the wrapper runs, or `nice rm -rf .git`
/// walks past a denial the literal spelling gets (D205).
#[test]
fn a_wrapped_destructive_command_is_denied_in_every_mode() -> TestResult {
    let root = Scratch::new("yi-permission-wrapped")?;
    let (tree, common) = linked_layout(&root, false)?;
    let context = CatastrophicContext::detect(&tree);
    let session = SessionRules::new();
    for command in [
        format!("nice -n 5 rm -rf {}", common.display()),
        format!("stdbuf -o0 rm -rf {}", common.display()),
        format!("timeout 30 rm -rf {}", common.display()),
        format!("env GIT_DIR=x rm -rf {}", common.display()),
        format!("ionice -c 3 nice rm -rf {}", common.display()),
        format!("/usr/bin/time rm -rf {}", common.display()),
    ] {
        let call = bash_call(&command, "canonical");
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let decision = decide(&call, mode, &[], &session, &[], &context);
            assert!(
                matches!(decision, Decision::Deny { .. }),
                "{command} in {mode:?}: {decision:?}"
            );
        }
    }
    // A wrapper around something harmless is still judged on what it runs.
    let harmless = bash_call("nice -n 5 ls -la", "canonical");
    assert!(!matches!(
        decide(
            &harmless,
            PermissionMode::Auto,
            &[],
            &session,
            &[],
            &context
        ),
        Decision::Deny { .. }
    ));
    Ok(())
}
