use serde_json::{Value, json};
use yi_runtime::PermissionMode;

use super::{
    Case, Gate, TestResult, asking, command, logging, read_lines, settings, touching, write_json,
};

fn refusing(text: &str) -> Value {
    command(&format!("echo '{text}' >&2; exit 2"))
}

#[tokio::test]
async fn a_plugin_listed_by_its_install_root_runs_its_hooks() -> TestResult {
    let case = Case::new("plugin-dir", json!({}), true)?;
    let plugin = case.path("installs/guard");
    let seen = case.path("root-seen");
    let hook = command(&format!(
        "printf '%s' \"$CLAUDE_PLUGIN_ROOT\" > '{}'; echo 'plugin says no' >&2; exit 2",
        seen.display()
    ));
    write_json(
        &plugin.join("hooks/hooks.json"),
        &settings("PreToolUse", "Bash", vec![hook]),
    )?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe { std::env::set_var("YI_CLAUDE_HOOKS_FILES", &plugin) };
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(outcome.text.contains("plugin says no"), "{}", outcome.text);
    assert_eq!(std::fs::read_to_string(&seen)?, plugin.to_string_lossy());
    Ok(())
}

#[tokio::test]
async fn an_inherited_project_dir_is_the_project_of_an_agent_in_a_worktree() -> TestResult {
    let case = Case::new("project-dir", json!({}), true)?;
    let project = case.path("project");
    let seen = case.path("project-seen");
    let hook = command(&format!(
        "printf '%s' \"$CLAUDE_PROJECT_DIR\" > '{}'",
        seen.display()
    ));
    write_json(
        &project.join(".claude/settings.json"),
        &settings("PreToolUse", "Bash", vec![hook]),
    )?;
    case.trust(&project)?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", &project) };
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(!outcome.text.contains("yi trust"), "{}", outcome.text);
    let seen = std::fs::read_to_string(&seen)?;
    assert_eq!(seen, project.to_string_lossy(), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn an_if_rule_runs_its_hook_on_a_match_and_on_any_input_it_cannot_judge() -> TestResult {
    let mut rm = refusing("no rm");
    rm["if"] = Value::from("Bash(rm *)");
    let mut ts = refusing("no ts");
    ts["if"] = Value::from("Write(*.ts)");
    let config = json!({"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [rm]},
        {"matcher": "Write", "hooks": [ts]},
    ]}});
    let case = Case::new("if", config, true)?;
    let cases = [
        ("bash", json!({"command": "ls"}), false),
        ("bash", json!({"command": "ls && rm -f gone"}), true),
        ("bash", json!({"command": "FOO=1 rm -f gone"}), true),
        ("bash", json!({"command": "echo $(rm -f gone)"}), true),
        ("write", json!({"path": "a.txt", "content": "x"}), false),
        ("write", json!({"path": "src/b.ts", "content": "x"}), true),
    ];
    for (tool, args, refused) in cases {
        let outcome = case.run(tool, args.clone()).await?;
        assert_eq!(outcome.is_error, refused, "{args}: {}", outcome.text);
    }
    Ok(())
}

#[tokio::test]
async fn a_project_file_without_hooks_needs_no_trust() -> TestResult {
    let case = Case::new("permissions-only", json!({}), true)?;
    let permissions = json!({"permissions": {"allow": ["Bash(ls:*)"]}});
    write_json(&case.cwd.join(".claude/settings.json"), &permissions)?;
    write_json(&case.cwd.join(".claude/settings.local.json"), &permissions)?;
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(!outcome.text.contains("yi trust"), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn a_hook_inherits_yis_environment_but_the_exporters_and_scrubbed_keys() -> TestResult {
    let case = Case::new("env", json!({}), true)?;
    let seen = case.path("env-seen");
    let hook = command(&format!(
        "printf '%s' \"${{OPENAI_API_KEY:-unset}}|${{OTEL_EXPORTER_OTLP_ENDPOINT:-unset}}\" > '{}'",
        seen.display()
    ));
    case.user(&settings("PreToolUse", "Bash", vec![hook]))?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "user-key");
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector");
    }
    for (scrub, expected) in [(false, "user-key|unset"), (true, "unset|unset")] {
        if scrub {
            // SAFETY: nextest isolates each test in its own process.
            unsafe { std::env::set_var("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1") };
        }
        case.run("bash", json!({"command": "printf ran"})).await?;
        assert_eq!(std::fs::read_to_string(&seen)?, expected, "scrub {scrub}");
    }
    Ok(())
}

#[tokio::test]
async fn matchers_refuse_what_they_do_not_name_and_one_bad_handler_spares_the_rest() -> TestResult {
    let case = Case::new("matchers-more", json!({}), true)?;
    let (first, last, wrong, count) = (
        case.cwd.join("first"),
        case.cwd.join("last"),
        case.cwd.join("wrong"),
        case.path("count"),
    );
    let counted = command(&format!("echo x >> '{}'", count.display()));
    let http = json!({"type": "http", "url": "http://localhost:1/hook"});
    let powershell = json!({"type": "command", "command": "Write-Host", "shell": "powershell"});
    case.user(&json!({"hooks": {"PreToolUse": [
        {"matcher": "^Wri", "hooks": [touching(&wrong)]},
        {"matcher": "Bash", "hooks": [touching(&first), http, powershell, touching(&last)]},
        {"matcher": "Bash", "hooks": [counted.clone()]},
        {"matcher": "*", "hooks": [counted]},
    ]}}))?;
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(!wrong.exists(), "a ^Wri matcher ran for Bash");
    assert!(first.exists() && last.exists(), "{}", outcome.text);
    assert_eq!(
        std::fs::read_to_string(&count)?,
        "x\n",
        "an identical handler ran twice"
    );
    for notice in [
        "type http is not supported",
        "shell powershell is not supported",
    ] {
        assert!(outcome.text.contains(notice), "{}", outcome.text);
    }
    Ok(())
}

#[tokio::test]
async fn execution_options_do_not_duplicate_a_hook_command() -> TestResult {
    let case = Case::new("option-duplicates", json!({}), true)?;
    let count = case.path("option-count.jsonl");
    let hook = logging(&count);
    let mut handlers = vec![hook.clone()];
    for (key, value) in [
        ("timeout", json!(1)),
        ("onFailure", json!("block")),
        ("async", json!(true)),
        ("asyncRewake", json!(true)),
    ] {
        let mut changed = hook.clone();
        changed
            .as_object_mut()
            .ok_or("a command handler must be an object")?
            .insert(key.to_owned(), value);
        handlers.push(changed);
    }
    case.user(&settings("PreToolUse", "Bash", handlers))?;
    case.run("bash", json!({"command": "printf ran"})).await?;
    assert_eq!(
        read_lines(&count)?.len(),
        1,
        "execution options duplicated one hook command"
    );
    Ok(())
}

#[tokio::test]
async fn an_if_condition_cannot_hide_a_matching_duplicate_handler() -> TestResult {
    let case = Case::new("conditional-duplicates", json!({}), true)?;
    let ran = case.path("conditional-ran");
    let hook = super::touching(&ran);
    let mut other = hook.clone();
    other["if"] = Value::from("Bash(rm *)");
    let mut matching = hook;
    matching["if"] = Value::from("Bash(printf *)");
    case.user(&json!({"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [other]},
        {"matcher": "Bash", "hooks": [matching]},
    ]}}))?;
    case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(
        ran.exists(),
        "the first condition suppressed the matching handler"
    );
    Ok(())
}

#[tokio::test]
async fn permission_mode_names_ask_and_auto_as_claude_code_does() -> TestResult {
    let case = Case::new("modes", json!({}), true)?;
    let log = case.path("modes.jsonl");
    case.user(&settings("PreToolUse", "Bash", vec![logging(&log)]))?;
    for mode in [PermissionMode::Ask, PermissionMode::Auto] {
        let gate = Gate { ..asking(mode) };
        case.run_gated("bash", json!({"command": "printf ran"}), gate)
            .await?;
    }
    let lines = read_lines(&log)?;
    let modes: Vec<&Value> = lines.iter().map(|line| &line["permission_mode"]).collect();
    assert_eq!(modes, [&json!("default"), &json!("acceptEdits")]);
    Ok(())
}

#[tokio::test]
async fn plain_matchers_are_unanchored_case_insensitive_regexes() -> TestResult {
    let case = Case::new("plain-regex", json!({}), true)?;
    let (prefix, lowercase) = (case.path("prefix-ran"), case.path("lower-ran"));
    let (comma, piped, grouped) = (
        case.path("comma-ran"),
        case.path("pipe-ran"),
        case.path("grouped-ran"),
    );
    case.user(&json!({"hooks": {"PreToolUse": [
        {"matcher": "mcp__yi", "hooks": [super::touching(&prefix)]},
        {"matcher": "bash", "hooks": [super::touching(&lowercase)]},
        {"matcher": "Edit, Write", "hooks": [super::touching(&comma)]},
        {"matcher": "Bash | Edit", "hooks": [super::touching(&piped)]},
        {"matcher": "^(Bash|Edit)$", "hooks": [super::touching(&grouped)]},
    ]}}))?;
    case.run("ipython", json!({"code": "1 + 1"})).await?;
    case.run("bash", json!({"command": "printf ran"})).await?;
    case.run("edit", json!({"patch": "[a#ZZZZ]"})).await?;
    assert!(prefix.exists(), "an unanchored MCP matcher did not run");
    assert!(lowercase.exists(), "a case-insensitive matcher did not run");
    assert!(comma.exists(), "a comma-separated matcher did not run");
    assert!(piped.exists(), "a space-padded alternation did not run");
    assert!(grouped.exists(), "a grouped regex alternation did not run");
    Ok(())
}
