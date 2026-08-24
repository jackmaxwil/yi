#![forbid(unsafe_code)]

pub mod args;
pub mod callargs;
pub mod client;
pub mod config;
pub mod grep;
pub mod output;
pub mod sessions;

use std::path::PathBuf;

use serde_json::{Value, json};
use yi_types::mcp::{McpServerSpec, McpSessionState};

use args::{Command, Flags, SchemaMode, SessionOp};
use client::Op;
use sessions::SessionsStore;

pub const SKILL: &str = include_str!("SKILL.md");

const HELP: &str = "yi mcp — MCP client (stateless: each command connects, runs, exits)

Commands:
  connect <server> [@session]   connect and snapshot tools (<server>: config
                                entry name, <file>:<entry>, or config path)
  close @session                mark a session disconnected
  restart @session              reconnect and refresh the snapshot
  grep <pattern> [-m <n>]       search cached tools/instructions, all sessions
  @session tools-list           list tools
  @session tools-get <name> [--schema <file>] [--schema-mode strict|compatible]
  @session tools-call <name> [key:=value ... | '<json>' | <stdin]
  @session resources-list | prompts-list | ping | close | restart | grep <pattern>
  help --skill                  print the agent-facing skill document

Options: --json (MCP-spec JSON on stdout), --max-chars <n> (truncate non-JSON)
Exit codes: 0 ok, 1 grep-no-match, 2 usage, 3 server failure";

fn fail(message: &str, code: i32) -> i32 {
    eprintln!("error: {message}");
    code
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn snapshot_from(connected: &Value) -> Value {
    json!({
        "tools": connected.get("tools").cloned().unwrap_or(Value::Array(Vec::new())),
        "instructions": connected.get("instructions").cloned().unwrap_or(Value::Null),
    })
}

fn do_connect(
    store: &mut SessionsStore,
    server: &str,
    session: Option<String>,
    flags: &Flags,
) -> i32 {
    let (entry_name, spec) = match config::resolve_server(server, &home_dir()) {
        Ok(found) => found,
        Err(error) => return fail(&error, 2),
    };
    let name = session.unwrap_or_else(|| sessions::default_session_name(&entry_name));
    let mut record = sessions::new_record(&name, spec.clone());
    let _ = store.upsert(&record);
    match client::one_shot(&spec, Op::Discover) {
        Ok(connected) => {
            record.state = McpSessionState::Live;
            record.protocol_version = connected
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned);
            record.server_name = connected
                .pointer("/serverInfo/name")
                .and_then(Value::as_str)
                .map(str::to_owned);
            record.instructions = connected
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            record.updated_at = sessions::now_ms();
            if let Err(error) = store.upsert(&record) {
                return fail(&error, 3);
            }
            let _ = store.write_snapshot(&name, &snapshot_from(&connected));
            output::emit(
                &json!({"session": format!("@{name}"), "state": "live", "server": connected}),
                flags.json,
                flags.max_chars,
            );
            0
        }
        Err(error) => {
            let _ = store.set_state(&name, McpSessionState::Disconnected);
            fail(&format!("@{name}: {error}"), 3)
        }
    }
}

fn spec_of(store: &SessionsStore, session: &str) -> Result<McpServerSpec, String> {
    store
        .get(session)
        .map(|record| record.spec)
        .ok_or_else(|| format!("unknown session @{session} (yi mcp connect <server> @{session})"))
}

fn compare_schema(
    actual_tool: Option<&Value>,
    schema: &std::path::Path,
    mode: SchemaMode,
) -> Result<(), String> {
    let Some(actual) = actual_tool else {
        return Err("tool not found".to_owned());
    };
    let expected_text = std::fs::read_to_string(schema)
        .map_err(|error| format!("cannot read {}: {error}", schema.display()))?;
    let expected: Value = serde_json::from_str(&expected_text)
        .map_err(|error| format!("bad JSON in {}: {error}", schema.display()))?;
    output::schema_compatible(&expected, actual, mode == SchemaMode::Strict)
}

fn find_tool(listing: &Value, name: &str) -> Option<Value> {
    listing
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

fn do_session_op(store: &mut SessionsStore, session: &str, op: SessionOp, flags: &Flags) -> i32 {
    let spec = match spec_of(store, session) {
        Ok(spec) => spec,
        Err(error) => return fail(&error, 2),
    };
    let run = |store: &mut SessionsStore, client_op: Op| -> Result<Value, String> {
        let result = client::one_shot(&spec, client_op);
        let state = if result.is_ok() {
            McpSessionState::Live
        } else {
            McpSessionState::Disconnected
        };
        let _ = store.set_state(session, state);
        result
    };
    let outcome = match op {
        SessionOp::ToolsList => run(store, Op::ToolsList).inspect(|listing| {
            let _ = store.write_snapshot(
                session,
                &json!({"tools": listing.get("tools").cloned().unwrap_or_default()}),
            );
        }),
        SessionOp::ToolsGet { name, schema, mode } => {
            run(store, Op::ToolsList).and_then(|listing| {
                let tool = find_tool(&listing, &name);
                if let Some(schema) = schema {
                    compare_schema(tool.as_ref(), &schema, mode)?;
                }
                tool.ok_or_else(|| format!("tool {name} not found"))
            })
        }
        SessionOp::ToolsCall { name, args } => match callargs::resolve(&args) {
            Ok(arguments) => run(store, Op::ToolsCall { name, arguments }),
            Err(error) => return fail(&error, 2),
        },
        SessionOp::ResourcesList => run(store, Op::ResourcesList),
        SessionOp::PromptsList => run(store, Op::PromptsList),
        SessionOp::Ping => run(store, Op::Ping),
        SessionOp::Grep { pattern } => {
            return do_grep(store, &[session.to_owned()], &pattern, None, flags);
        }
        SessionOp::Close | SessionOp::Restart => return fail("unreachable", 2),
    };
    match outcome {
        Ok(value) => {
            output::emit(&value, flags.json, flags.max_chars);
            0
        }
        Err(error) => fail(&format!("@{session}: {error}"), 3),
    }
}

fn do_grep(
    store: &SessionsStore,
    sessions: &[String],
    pattern: &str,
    max_results: Option<usize>,
    flags: &Flags,
) -> i32 {
    match grep::grep_sessions(store, sessions, pattern, max_results) {
        Some(results) => {
            output::emit(&results, flags.json, flags.max_chars);
            0
        }
        None => {
            if flags.json {
                println!("[]");
            } else {
                println!("no matches");
            }
            1
        }
    }
}

/// Entry point for the `yi mcp` subcommand. The `mcp.enabled` gate lives in
/// the caller (yi-cli) — this crate assumes it was consulted.
pub fn run(raw_args: &[String]) -> i32 {
    let parsed = match args::parse(raw_args) {
        Ok(parsed) => parsed,
        Err(error) => return fail(&format!("{error}\n\n{HELP}"), 2),
    };
    let mut store = SessionsStore::open(sessions::mcp_root(&home_dir()));
    match parsed.command {
        Command::Help => {
            println!("{HELP}");
            0
        }
        Command::Skill => {
            println!("{SKILL}");
            0
        }
        Command::Connect { server, session } => {
            do_connect(&mut store, &server, session, &parsed.flags)
        }
        Command::Close { session } => {
            match store.set_state(&session, McpSessionState::Disconnected) {
                Ok(()) => {
                    output::emit(
                        &json!({"session": format!("@{session}"), "state": "disconnected"}),
                        parsed.flags.json,
                        parsed.flags.max_chars,
                    );
                    0
                }
                Err(error) => fail(&error, 2),
            }
        }
        Command::Restart { session } => match store.get(&session) {
            Some(record) => {
                let server = record.name.clone();
                let _ = store.set_state(&session, McpSessionState::Connecting);
                let spec = record.spec;
                match client::one_shot(&spec, Op::Discover) {
                    Ok(connected) => {
                        let _ = store.set_state(&session, McpSessionState::Live);
                        let _ = store.write_snapshot(&session, &snapshot_from(&connected));
                        output::emit(
                            &json!({"session": format!("@{server}"), "state": "live"}),
                            parsed.flags.json,
                            parsed.flags.max_chars,
                        );
                        0
                    }
                    Err(error) => {
                        let _ = store.set_state(&session, McpSessionState::Expired);
                        fail(&format!("@{session}: {error}"), 3)
                    }
                }
            }
            None => fail(&format!("unknown session @{session}"), 2),
        },
        Command::Grep {
            pattern,
            max_results,
        } => {
            let names = store.names();
            do_grep(&store, &names, &pattern, max_results, &parsed.flags)
        }
        Command::Session { session, op } => do_session_op(&mut store, &session, op, &parsed.flags),
    }
}
