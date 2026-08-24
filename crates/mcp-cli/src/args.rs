use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaMode {
    #[default]
    Compatible,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallArgs {
    Pairs(Vec<String>),
    Json(String),
    Stdin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionOp {
    ToolsList,
    ToolsGet {
        name: String,
        schema: Option<PathBuf>,
        mode: SchemaMode,
    },
    ToolsCall {
        name: String,
        args: CallArgs,
    },
    ResourcesList,
    PromptsList,
    Ping,
    Close,
    Restart,
    Grep {
        pattern: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Skill,
    Connect {
        server: String,
        session: Option<String>,
    },
    Close {
        session: String,
    },
    Restart {
        session: String,
    },
    Grep {
        pattern: String,
        max_results: Option<usize>,
    },
    Session {
        session: String,
        op: SessionOp,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub json: bool,
    pub max_chars: Option<usize>,
}

pub struct Parsed {
    pub command: Command,
    pub flags: Flags,
}

type SplitArgs = (Vec<String>, Flags, Vec<(String, String)>);

fn split_flags(args: &[String]) -> Result<SplitArgs, String> {
    let mut positional = Vec::new();
    let mut flags = Flags::default();
    let mut options: Vec<(String, String)> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--json" => flags.json = true,
            "--max-chars" | "--max-results" | "--schema" | "--schema-mode" | "-m" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                options.push((arg.clone(), value.clone()));
                index += 1;
            }
            "--help" | "-h" => positional.insert(0, "help".to_owned()),
            "--skill" => positional.push("--skill".to_owned()),
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}"));
            }
            _ => positional.push(arg.clone()),
        }
        index += 1;
    }
    if let Some((_, value)) = options.iter().find(|(name, _)| name == "--max-chars") {
        flags.max_chars = Some(
            value
                .parse()
                .map_err(|_| format!("--max-chars: not a number: {value}"))?,
        );
    }
    Ok((positional, flags, options))
}

fn option<'a>(options: &'a [(String, String)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn schema_options(options: &[(String, String)]) -> Result<(Option<PathBuf>, SchemaMode), String> {
    let schema = option(options, "--schema").map(PathBuf::from);
    let mode = match option(options, "--schema-mode") {
        None | Some("compatible") => SchemaMode::Compatible,
        Some("strict") => SchemaMode::Strict,
        Some(other) => {
            return Err(format!(
                "--schema-mode: unknown mode {other} (strict | compatible)"
            ));
        }
    };
    Ok((schema, mode))
}

fn call_args(rest: &[String]) -> CallArgs {
    if rest.is_empty() {
        return CallArgs::Stdin;
    }
    if rest.len() == 1 && rest[0].trim_start().starts_with(['{', '[']) {
        return CallArgs::Json(rest[0].clone());
    }
    CallArgs::Pairs(rest.to_vec())
}

fn session_op(words: &[String], options: &[(String, String)]) -> Result<SessionOp, String> {
    let op = words.first().map(String::as_str).unwrap_or("");
    match op {
        "tools-list" => Ok(SessionOp::ToolsList),
        "tools-get" => {
            let name = words
                .get(1)
                .ok_or("tools-get requires a tool name")?
                .clone();
            let (schema, mode) = schema_options(options)?;
            Ok(SessionOp::ToolsGet { name, schema, mode })
        }
        "tools-call" => {
            let name = words
                .get(1)
                .ok_or("tools-call requires a tool name")?
                .clone();
            Ok(SessionOp::ToolsCall {
                name,
                args: call_args(&words[2..]),
            })
        }
        "resources-list" => Ok(SessionOp::ResourcesList),
        "prompts-list" => Ok(SessionOp::PromptsList),
        "ping" => Ok(SessionOp::Ping),
        "close" => Ok(SessionOp::Close),
        "restart" => Ok(SessionOp::Restart),
        "grep" => Ok(SessionOp::Grep {
            pattern: words.get(1).ok_or("grep requires a pattern")?.clone(),
        }),
        "" => Err("missing session subcommand (try tools-list)".to_owned()),
        other => Err(format!("unknown session subcommand {other}")),
    }
}

pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let (positional, flags, options) = split_flags(args)?;
    let first = positional.first().map(String::as_str).unwrap_or("help");
    let command = if let Some(session) = first.strip_prefix('@') {
        let op = session_op(&positional[1..], &options)?;
        match op {
            SessionOp::Close => Command::Close {
                session: session.to_owned(),
            },
            SessionOp::Restart => Command::Restart {
                session: session.to_owned(),
            },
            other => Command::Session {
                session: session.to_owned(),
                op: other,
            },
        }
    } else {
        match first {
            "help" => {
                if positional.get(1).map(String::as_str) == Some("--skill")
                    || positional.iter().any(|word| word == "--skill")
                {
                    Command::Skill
                } else {
                    Command::Help
                }
            }
            "skill" => Command::Skill,
            "connect" => {
                let server = positional
                    .get(1)
                    .ok_or("connect requires a server (name, config:entry, or path)")?
                    .clone();
                let session = positional
                    .get(2)
                    .map(|word| {
                        word.strip_prefix('@')
                            .map(str::to_owned)
                            .ok_or_else(|| format!("session name must start with @: {word}"))
                    })
                    .transpose()?;
                Command::Connect { server, session }
            }
            "close" | "restart" => {
                let session = positional
                    .get(1)
                    .and_then(|word| word.strip_prefix('@'))
                    .ok_or_else(|| format!("{first} requires an @session"))?
                    .to_owned();
                if first == "close" {
                    Command::Close { session }
                } else {
                    Command::Restart { session }
                }
            }
            "grep" => {
                let pattern = positional.get(1).ok_or("grep requires a pattern")?.clone();
                let max_results = option(&options, "-m")
                    .or_else(|| option(&options, "--max-results"))
                    .map(|value| {
                        value
                            .parse()
                            .map_err(|_| format!("--max-results: not a number: {value}"))
                    })
                    .transpose()?;
                Command::Grep {
                    pattern,
                    max_results,
                }
            }
            other => return Err(format!("unknown command {other} (try help)")),
        }
    };
    Ok(Parsed { command, flags })
}
