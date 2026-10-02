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

/// The field list a schema block renders: one line per property, in the schema's order, with its
/// type, whether it is required, and its description.
fn render_fields(schema: &serde_json::Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
        .unwrap_or_default();
    let kind = |field: &serde_json::Value| match field["type"].as_str() {
        Some("array") => format!(
            "list of {}",
            field["items"]["type"].as_str().unwrap_or("value")
        ),
        Some(name) => name.to_owned(),
        None => "value".to_owned(),
    };
    let mut out = Vec::new();
    for (name, field) in schema["properties"].as_object().into_iter().flatten() {
        let mut facts = vec![kind(field)];
        if required.contains(&name.as_str()) {
            facts.push("required".to_owned());
        }
        if let Some(values) = field["enum"].as_array() {
            let values: Vec<String> = values
                .iter()
                .map(|v| format!("`{}`", v.as_str().unwrap_or("")))
                .collect();
            facts.push(format!("one of {}", values.join(", ")));
        }
        let description = field["description"].as_str().unwrap_or("");
        let line = format!("- `{name}` ({})", facts.join(", "));
        out.push(if description.is_empty() {
            line
        } else {
            format!("{line}: {description}")
        });
    }
    out.join("\n")
}

const BLOCK_OPEN: &str = "<!-- yi:schema ";
const BLOCK_CLOSE: &str = "<!-- /yi:schema -->";

/// Every schema block in `text`, re-rendered from the live schema it names.
fn rendered(text: &str) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let mut out = String::new();
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(BLOCK_OPEN) {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let header_end = from.find("-->").ok_or("an unclosed block header")?;
        let header = from
            .get(BLOCK_OPEN.len()..header_end)
            .ok_or("header")?
            .trim();
        let (tool, pointer) = header
            .split_once(' ')
            .ok_or("a header names a tool and a pointer")?;
        let schema = match tool {
            "plan" => yi_runtime::plan::tool::schema(),
            "todo" => yi_runtime::todo::tool::schema(),
            other => return Err(format!("no schema named {other}").into()),
        };
        let at_pointer = schema
            .pointer(pointer)
            .ok_or(format!("{tool} has no {pointer}"))?;
        let close = from.find(BLOCK_CLOSE).ok_or("a block without its close")?;
        out.push_str(from.get(..header_end + 3).ok_or("header")?);
        out.push('\n');
        out.push_str(&render_fields(at_pointer));
        out.push('\n');
        out.push_str(BLOCK_CLOSE);
        blocks.push(format!("{tool} {pointer}"));
        rest = from.get(close + BLOCK_CLOSE.len()..).ok_or("tail")?;
    }
    out.push_str(rest);
    Ok((out, blocks))
}

/// Dies with a prompt or skill teaching fields its tool does not take: the plan skill still said
/// title, acceptance, check and deps after the orchestrate protocol was fixed by hand.
#[test]
fn every_schema_block_is_the_schemas_own_rendering() -> TestResult {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.ancestors().nth(2).ok_or("the repository root")?;
    let skills = root.join("skills");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(manifest.join("src/prompts"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    for set in std::fs::read_dir(&skills)?.flatten() {
        for skill in std::fs::read_dir(set.path())?.flatten() {
            let manifest = skill.path().join("SKILL.md");
            if manifest.is_file() {
                files.push(manifest);
            }
        }
    }
    let bless = std::env::var_os("YI_BLESS").is_some();
    let mut stale = Vec::new();
    let mut found = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)?;
        let (fresh, blocks) = rendered(&text)?;
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        found.extend(blocks.into_iter().map(|block| format!("{name}: {block}")));
        if fresh != text {
            if bless {
                std::fs::write(&path, fresh)?;
            } else {
                stale.push(name);
            }
        }
    }
    assert!(
        stale.is_empty(),
        "stale schema blocks in {stale:?}; run `just schema-blocks`"
    );
    for owed in [
        "crates/runtime/src/prompts/orchestrate.md: plan /properties/todos/items",
        "skills/yi/plan/SKILL.md: plan /properties/todos/items",
        "skills/yi/review/SKILL.md: plan /properties/todos/items/properties/contract/properties/items/items",
    ] {
        assert!(
            found.iter().any(|block| block == owed),
            "missing block {owed}; found {found:?}"
        );
    }
    Ok(())
}
