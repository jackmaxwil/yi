#![forbid(unsafe_code)]

pub mod args;
pub mod callargs;
pub mod client;
pub mod config;
pub mod grep;
pub mod http;
pub mod oauth;
pub mod output;
pub mod profiles;
pub mod sessions;
pub mod stdio;
pub mod tokens;

use std::path::PathBuf;

use serde_json::{Value, json};
use yi_types::mcp::{McpServerSpec, McpSessionState};

use args::{Command, Flags, SchemaMode, SessionOp};
use client::Op;
use profiles::ProfilesStore;
use sessions::SessionsStore;
use tokens::{TokenStore, Tokens};

pub const SKILL: &str = include_str!("SKILL.md");

const HELP: &str = "yi mcp — MCP client (stateless: each command connects, runs, exits)

Commands:
  connect <server> [@session]   connect and snapshot tools (<server>: config
                                entry name, <file>:<entry>, config path, or URL)
                                [--profile <name> | --no-profile]
  login <server-url>            OAuth 2.1 + PKCE login; tokens go to the OS
                                keychain (mcp.tokenStore config: keychain|file)
                                [--profile <name>] [--scopes a,b] [--client-id id] [--no-browser]
  logout <server-url>           delete stored tokens [--profile <name>] [--purge]
  close @session                mark a session disconnected
  restart @session              reconnect and refresh the snapshot
  grep <pattern> [-m <n>]       search cached tools/instructions, all sessions
  @session tools-list           list tools
  @session tools-get <name> [--schema <file>] [--schema-mode strict|compatible]
  @session tools-call <name> [key:=value ... | '<json>' | <stdin]
  @session resources-read <uri>  read one resource
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

/// Invariant: `run` fills this before dispatch. yi-cli owns the one strict config read (§17.1),
/// so this crate never opens the file; a second reader could disagree with the first.
static TOKEN_STORE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn token_store() -> Tokens {
    let configured = TOKEN_STORE.get().and_then(Option::as_deref);
    Tokens::new(
        TokenStore::from_config(configured),
        sessions::mcp_root(&home_dir()),
    )
}

/// Bearer token for an HTTP spec via its OAuth profile; stdio needs none. Never triggers a
/// login: a missing or dead credential is an error that names the login command.
fn auth_for(spec: &McpServerSpec, profile_name: Option<&str>) -> Result<Option<String>, String> {
    let McpServerSpec::Http { url } = spec else {
        return Ok(None);
    };
    let profiles = ProfilesStore::open(sessions::mcp_root(&home_dir()));
    let name = profile_name.unwrap_or("default");
    let Some(profile) = profiles.get(name, url) else {
        return Ok(None);
    };
    let agent = ureq::AgentBuilder::new().build();
    oauth::access_token(&agent, &profile, &token_store()).map(Some)
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
    profile: Option<&str>,
    no_profile: bool,
    flags: &Flags,
) -> i32 {
    let (entry_name, spec) = match config::resolve_server(server, &home_dir()) {
        Ok(found) => found,
        Err(error) => return fail(&error, 2),
    };
    let auth = if no_profile {
        None
    } else {
        match auth_for(&spec, profile) {
            Ok(token) => token,
            Err(error) => return fail(&error, 3),
        }
    };
    let name = session.unwrap_or_else(|| sessions::default_session_name(&entry_name));
    let mut record = sessions::new_record(&name, spec.clone());
    if let Some(profile) = profile {
        record
            .extra
            .insert("profile".to_owned(), Value::String(profile.to_owned()));
    }
    if no_profile {
        record
            .extra
            .insert("noProfile".to_owned(), Value::Bool(true));
    }
    let _ = store.upsert(&record);
    match client::one_shot_with_auth(&spec, Op::Discover, auth) {
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
            let state = if error.contains(http::UNAUTHORIZED_MARKER) {
                McpSessionState::Unauthorized
            } else {
                McpSessionState::Disconnected
            };
            let _ = store.set_state(&name, state);
            if state == McpSessionState::Unauthorized {
                return fail(
                    &format!("@{name}: unauthorized. Run: yi mcp login {server}"),
                    3,
                );
            }
            fail(&format!("@{name}: {error}"), 3)
        }
    }
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
    let record = match store.get(session) {
        Some(record) => record,
        None => {
            return fail(
                &format!("unknown session @{session} (yi mcp connect <server> @{session})"),
                2,
            );
        }
    };
    let spec = record.spec.clone();
    let profile_name = record
        .extra
        .get("profile")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let skip_auth = record
        .extra
        .get("noProfile")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let auth = if skip_auth {
        None
    } else {
        match auth_for(&spec, profile_name.as_deref()) {
            Ok(token) => token,
            Err(error) => return fail(&error, 3),
        }
    };
    let run = |store: &mut SessionsStore, client_op: Op| -> Result<Value, String> {
        let result = client::one_shot_with_auth(&spec, client_op, auth.clone());
        let state = match &result {
            Ok(_) => McpSessionState::Live,
            Err(error) if error.contains(http::UNAUTHORIZED_MARKER) => {
                McpSessionState::Unauthorized
            }
            Err(_) => McpSessionState::Disconnected,
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
        SessionOp::ResourcesRead { uri } => run(store, Op::ResourcesRead { uri }),
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
pub fn run(raw_args: &[String], configured_token_store: Option<&str>) -> i32 {
    let _already_set = TOKEN_STORE.set(configured_token_store.map(str::to_owned));
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
        Command::Connect {
            server,
            session,
            profile,
            no_profile,
        } => do_connect(
            &mut store,
            &server,
            session,
            profile.as_deref(),
            no_profile,
            &parsed.flags,
        ),
        Command::Login {
            server,
            profile,
            scopes,
            client_id,
            no_browser,
        } => {
            let agent = ureq::AgentBuilder::new().build();
            let options = oauth::LoginOptions {
                profile: profile.clone(),
                scopes,
                client_id,
                no_browser,
            };
            match oauth::login(&agent, &server, &options) {
                Ok((record, credential)) => {
                    let key = oauth::profile_key(&record.name, &record.server_url);
                    let mut profiles = ProfilesStore::open(sessions::mcp_root(&home_dir()));
                    if let Err(error) = profiles
                        .upsert(&record)
                        .and_then(|()| token_store().save(&key, &credential))
                    {
                        return fail(&error, 3);
                    }
                    output::emit(
                        &json!({
                            "profile": record.name,
                            "server": record.server_url,
                            "issuer": record.issuer,
                            "state": "authorized",
                        }),
                        parsed.flags.json,
                        parsed.flags.max_chars,
                    );
                    0
                }
                Err(error) => fail(&error, 3),
            }
        }
        Command::Logout {
            server,
            profile,
            purge,
        } => {
            let key = oauth::profile_key(&profile, &server);
            if let Err(error) = token_store().delete(&key) {
                return fail(&error, 3);
            }
            if purge {
                let mut profiles = ProfilesStore::open(sessions::mcp_root(&home_dir()));
                if let Err(error) = profiles.remove(&profile, &server) {
                    return fail(&error, 3);
                }
            }
            output::emit(
                &json!({"profile": profile, "server": server, "state": "logged_out"}),
                parsed.flags.json,
                parsed.flags.max_chars,
            );
            0
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
