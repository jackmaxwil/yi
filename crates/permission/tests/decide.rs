use std::error::Error;
use std::path::PathBuf;

use yi_permission::{
    CatastrophicContext, ConfigRule, ConfigRuleAction, Decision, Hold, HoldSource, ParseOutcome,
    PermissionMode, SessionRules, ToolCall, canonical_command_identity, decide, is_catastrophic,
    lexical_normalize, parse_command,
};
use yi_types::permission::{RuleDecision, RuleKind, SessionPermissionState};

type TestResult = Result<(), Box<dyn Error>>;

fn context() -> CatastrophicContext {
    CatastrophicContext {
        home_dir: Some(PathBuf::from("/home/user")),
        working_dir: Some(PathBuf::from("/home/user/project")),
        workspace_git: Some(PathBuf::from("/home/user/project/.git")),
    }
}

fn bash_call<'a>(command: &'a str, canonical: &'a str) -> ToolCall<'a> {
    ToolCall {
        tool_name: "bash",
        reads_only: false,
        irreversible: true,
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
fn mode_fallbacks_match_the_fx_gate() -> TestResult {
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
