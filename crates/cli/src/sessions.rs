use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use yi_runtime::session_store::{
    BranchBounds, EntryOrder, EntryQuery, JsonlRepo, SessionMetadata, SessionRepo, lock_session,
    validate_session_id,
};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};

pub struct Options {
    pub session_dir: PathBuf,
    pub cwd: String,
    pub json: bool,
}

pub fn run(subcommand: &str, options: &Options) -> i32 {
    let mut repo = JsonlRepo::new(options.session_dir.clone(), options.cwd.clone());
    let (verb, id) = split_subcommand(subcommand);
    match (verb, id) {
        ("list", _) => match repo.list() {
            Ok(listed) => {
                print_list(&listed, options.json);
                0
            }
            Err(error) => fail(&error.to_string()),
        },
        ("show", Some(id)) => match repo.open(id) {
            Ok(store) => {
                let session = lock_session(&store);
                let entries = session.find_entries_on_branch(
                    "main",
                    &EntryQuery {
                        order: EntryOrder::OldestFirst,
                        ..EntryQuery::default()
                    },
                    &BranchBounds::default(),
                );
                match entries {
                    Ok(entries) => {
                        print_show(session.metadata(), &entries, options.json);
                        0
                    }
                    Err(error) => fail(&error.to_string()),
                }
            }
            Err(error) => fail(&error.to_string()),
        },
        ("rm", Some(id)) => match repo
            .delete(id)
            .map_err(|error| error.to_string())
            .and_then(|()| remove_board(&options.session_dir, id))
        {
            Ok(()) => {
                if options.json {
                    print_json(&json!({ "removed": id }));
                } else {
                    println!("removed {id}");
                }
                0
            }
            Err(error) => fail(&error),
        },
        ("show" | "rm", None) => usage(),
        _ => usage(),
    }
}

pub(crate) fn board_dir(session_dir: &Path, id: &str) -> PathBuf {
    session_dir.join("family").join(id)
}

fn remove_board(session_dir: &Path, id: &str) -> Result<(), String> {
    validate_session_id(id).map_err(|error| error.to_string())?;
    let kernel = session_dir.join("kernels").join(id);
    let clock = session_dir.join("schedules").join(id);
    [board_dir(session_dir, id), kernel, clock]
        .into_iter()
        .try_for_each(|dir| match std::fs::remove_dir_all(dir) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        })
}

fn split_subcommand(subcommand: &str) -> (&str, Option<&str>) {
    let mut parts = subcommand.split_whitespace();
    let verb = parts.next().unwrap_or_default();
    (verb, parts.next())
}

fn usage() -> i32 {
    eprintln!("usage: yi sessions list | yi sessions show <id> | yi sessions rm <id>");
    2
}

fn fail(message: &str) -> i32 {
    eprintln!("error: {message}");
    1
}

fn print_json(value: &Value) {
    if let Ok(line) = serde_json::to_string(value) {
        println!("{line}");
    }
}

fn print_list(listed: &[SessionMetadata], json: bool) {
    if json {
        print_json(&json!(listed));
        return;
    }
    println!("{}", yi_runtime::slash::sessions_listing(listed));
}

fn print_show(metadata: &SessionMetadata, entries: &[Entry], json: bool) {
    if json {
        print_json(&json!({ "session": metadata, "entries": entries }));
        return;
    }
    println!("session {}", metadata.id);
    if let Some(parent) = &metadata.parent_session_id {
        println!("forked from {parent}");
    }
    for entry in entries {
        if let Some((role, text)) = entry_line(entry) {
            println!("{role:>9}  {text}");
        }
    }
}

fn entry_line(entry: &Entry) -> Option<(&'static str, String)> {
    let Entry::Message { message, .. } = entry else {
        return None;
    };
    match message {
        AgentMessage::User { content, .. } => Some(("user", one_line(&user_text(content)))),
        AgentMessage::Assistant { content, .. } => {
            Some(("assistant", one_line(&assistant_text(content))))
        }
        AgentMessage::ToolResult {
            tool_name,
            is_error,
            ..
        } => {
            let status = if *is_error { "error" } else { "ok" };
            Some(("tool", format!("{tool_name} ({status})")))
        }
        _ => None,
    }
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => assistant_text(blocks),
    }
}

fn assistant_text(content: &[Content]) -> String {
    let mut out = String::new();
    for block in content {
        match block {
            Content::Text { text, .. } => out.push_str(text),
            Content::ToolCall { name, .. } => {
                out.push('[');
                out.push_str(name);
                out.push(']');
            }
            _ => {}
        }
    }
    out
}

const PREVIEW_CHARS: usize = 96;

fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    match trimmed.char_indices().nth(PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}…", trimmed.get(..cut).unwrap_or_default()),
        None => trimmed.to_owned(),
    }
}

/// The newest session for the cwd, which `--continue` resumes.
pub fn latest_id(repo: &mut JsonlRepo) -> Option<String> {
    repo.list()
        .ok()?
        .into_iter()
        .max_by_key(|metadata| metadata.created_at)
        .map(|metadata| metadata.id)
}
