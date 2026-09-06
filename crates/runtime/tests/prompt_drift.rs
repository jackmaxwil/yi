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
