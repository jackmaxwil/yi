//! Drives the hand-rolled client (D71) against a server built on rmcp, so the
//! handshake, framing and envelope are checked against an implementation Yi
//! does not own.

use std::error::Error;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use yi_mcp_cli::client::{Op, one_shot};
use yi_types::mcp::McpServerSpec;

/// `cargo test` builds examples, and the test binary lives one directory over
/// from them, so the server is on disk by the time any test runs.
fn server_spec() -> Result<McpServerSpec, Box<dyn Error>> {
    let mut path: PathBuf = std::env::current_exe()?;
    path.pop();
    path.pop();
    path.push("examples");
    path.push("reference_server");
    if !path.exists() {
        return Err(format!("reference_server not built at {}", path.display()).into());
    }
    Ok(McpServerSpec::Stdio {
        command: path.display().to_string(),
        args: Vec::new(),
        env: Map::new(),
    })
}

#[test]
fn discover_negotiates_and_lists_the_server_tools() -> Result<(), Box<dyn Error>> {
    let found = one_shot(&server_spec()?, Op::Discover)?;
    assert_eq!(
        found.pointer("/protocolVersion").and_then(Value::as_str),
        Some("2025-11-25")
    );
    assert_eq!(
        found.pointer("/instructions").and_then(Value::as_str),
        Some("echoes what it is given")
    );
    assert_eq!(
        found.pointer("/tools/0/name").and_then(Value::as_str),
        Some("echo")
    );
    assert_eq!(
        found
            .pointer("/tools/0/inputSchema/required/0")
            .and_then(Value::as_str),
        Some("text")
    );
    Ok(())
}

#[test]
fn tools_list_returns_the_servers_own_result_object() -> Result<(), Box<dyn Error>> {
    let listing = one_shot(&server_spec()?, Op::ToolsList)?;
    assert_eq!(
        listing.pointer("/tools/0/name").and_then(Value::as_str),
        Some("echo")
    );
    Ok(())
}

#[test]
fn tools_call_round_trips_an_argument() -> Result<(), Box<dyn Error>> {
    let mut arguments = Map::new();
    arguments.insert("text".to_owned(), json!("through the wire"));
    let result = one_shot(
        &server_spec()?,
        Op::ToolsCall {
            name: "echo".to_owned(),
            arguments,
        },
    )?;
    assert_eq!(
        result.pointer("/content/0/text").and_then(Value::as_str),
        Some("through the wire")
    );
    Ok(())
}

#[test]
fn a_multiline_argument_survives_the_line_framing() -> Result<(), Box<dyn Error>> {
    let mut arguments = Map::new();
    arguments.insert("text".to_owned(), json!("one\ntwo\r\nthree"));
    let result = one_shot(
        &server_spec()?,
        Op::ToolsCall {
            name: "echo".to_owned(),
            arguments,
        },
    )?;
    assert_eq!(
        result.pointer("/content/0/text").and_then(Value::as_str),
        Some("one\ntwo\r\nthree")
    );
    Ok(())
}

#[test]
fn ping_reports_the_negotiated_version() -> Result<(), Box<dyn Error>> {
    let pong = one_shot(&server_spec()?, Op::Ping)?;
    assert_eq!(pong.pointer("/ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        pong.pointer("/protocolVersion").and_then(Value::as_str),
        Some("2025-11-25")
    );
    Ok(())
}

#[test]
fn an_unknown_tool_surfaces_the_servers_error() -> Result<(), Box<dyn Error>> {
    let outcome = one_shot(
        &server_spec()?,
        Op::ToolsCall {
            name: "nope".to_owned(),
            arguments: Map::new(),
        },
    );
    let message = outcome.err().ok_or("an unknown tool should not succeed")?;
    assert!(message.contains("tools/call failed"), "{message}");
    assert!(message.contains("no such tool"), "{message}");
    Ok(())
}
