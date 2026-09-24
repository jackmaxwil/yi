use std::error::Error;

use yi_runtime::{builtin_tools, doctrine_fragment, identity_fragment};

type TestResult = Result<(), Box<dyn Error>>;

/// identity.md once described grep as literal-only while the tool had been regex
/// for weeks: a prompt claim about a tool is checked against the tool, not trusted.
#[test]
fn identity_names_every_tool_the_session_registers() -> TestResult {
    let identity = identity_fragment();
    for tool in builtin_tools() {
        assert!(
            identity.contains(tool.name()),
            "identity.md does not name the {} tool",
            tool.name()
        );
    }
    for name in [
        yi_runtime::todo::tool::NAME,
        "plan",
        "get_context",
        "ipython",
    ] {
        assert!(identity.contains(name), "identity.md does not name {name}");
    }
    Ok(())
}

#[test]
fn every_flag_the_prompt_names_for_grep_is_in_its_schema() -> TestResult {
    let grep = builtin_tools()
        .into_iter()
        .find(|tool| tool.name() == "grep")
        .ok_or("no grep tool")?;
    let schema = grep.schema();
    let properties = schema
        .get("properties")
        .and_then(|value| value.as_object())
        .ok_or("grep schema has no properties")?;
    for flag in ["literal", "multiline", "type"] {
        assert!(
            identity_fragment().contains(flag),
            "identity.md no longer mentions grep's {flag}"
        );
        assert!(properties.contains_key(flag), "grep schema lost {flag}");
    }
    Ok(())
}

#[test]
fn doctrine_names_only_ops_the_todo_tool_accepts() -> TestResult {
    let schema = yi_runtime::todo::tool::schema();
    let ops: Vec<String> = schema["properties"]["op"]["enum"]
        .as_array()
        .ok_or("no op enum")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    for op in [
        "start", "done", "block", "unblock", "drop", "append", "view",
    ] {
        assert!(ops.contains(&op.to_owned()), "the todo tool lost `{op}`");
        assert!(
            doctrine_fragment().contains(&format!("`{op}")),
            "doctrine no longer teaches `{op}`"
        );
    }
    Ok(())
}

/// Dies with the text a model reads behind the engine: with a plan open the todo tool sold its
/// ops as the list's, and the plan schema offered a worktree with no contract (D223, D226).
#[test]
fn the_tool_text_says_the_plan_is_the_list_and_a_worktree_needs_a_contract() -> TestResult {
    let todo = yi_runtime::todo::tool::DESCRIPTION;
    assert!(
        todo.contains("While a plan is open the plan is the list"),
        "{todo}"
    );
    let schema = yi_runtime::plan::tool::schema();
    let todos = schema["properties"]["todos"]["description"]
        .as_str()
        .ok_or("no todos text")?;
    assert!(
        todos.contains("isolation worktree requires a contract"),
        "{todos}"
    );
    assert!(!todos.contains("completes unverified"), "{todos}");
    Ok(())
}

/// Listing formats in `read`'s description is a claim about the installed wheel, so the list is
/// checked against the wheel itself, both ways.
#[test]
fn read_names_exactly_the_formats_the_installed_wheel_converts() -> TestResult {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?);
    let python =
        yi_kernel::bootstrap::ensure_kernel_python(&yi_kernel::bootstrap::BootstrapOptions {
            on_progress: None,
            home: home.clone(),
            runtime_source_dir: yi_kernel::bootstrap::default_runtime_source_dir(),
            skills_source_dir: yi_kernel::bootstrap::default_skills_source_dir(),
            toolchain: None,
            venv_dir: None,
        })?;
    let probe = yi_tools::command(&python)
        .args([
            "-c",
            "import json, typing, anydoc; print(json.dumps([kind for kind in typing.get_args(anydoc.Format) if kind != 'csv']))",
        ])
        .output()?;
    let live: Vec<String> = serde_json::from_slice(&probe.stdout)?;
    let read = yi_runtime::builtin_tools_with(false, Some(yi_runtime::documents(&home)))
        .into_iter()
        .find(|tool| tool.name() == "read")
        .ok_or("no read tool")?;
    let claimed: Vec<String> = read
        .description()
        .split("converted to Markdown: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .map(|list| {
            let mut depth = 0_u32;
            let canonical: String = list
                .chars()
                .filter(|character| {
                    depth = match character {
                        '(' => depth.saturating_add(1),
                        ')' => depth.saturating_sub(1),
                        _ => depth,
                    };
                    depth == 0 && *character != ')'
                })
                .collect();
            canonical
                .split(", ")
                .map(|entry| entry.trim().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(!live.is_empty(), "the venv carries the converter");
    assert_eq!(claimed, live);
    Ok(())
}
