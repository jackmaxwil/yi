//! The remote port: the chat's session touches become daemon requests, and the `_yi/*`
//! updates the daemon streams decode into the chat's own events.

use serde_json::Value;
use yi_tui::hud::GoalView;
use yi_tui::{Answer, AskChoice, Reply, SessionPort};
use yi_types::acp::{
    AcpConfigOption, AcpExtensionUpdate, AcpPermissionOption, AcpPermissionOptionKind,
    AcpSessionUpdate, AcpUpdateParams,
};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::model::{Effort, Model};
use yi_types::subagent::ChildUpdate;
use yi_types::todo::TodoList;

#[derive(Debug, Clone, PartialEq)]
pub enum PortRequest {
    Rewind(String),
    New,
    Undo,
    Slash(String),
    Select(Box<Model>, Effort),
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSeq(pub u64);

impl EventSeq {
    pub fn follows(self, previous: Option<Self>) -> bool {
        previous.is_none_or(|previous| previous.0.checked_add(1) == Some(self.0))
    }
}

#[derive(Default)]
pub struct RemotePort {
    pub queue: Vec<PortRequest>,
    entries: Vec<Entry>,
    leaf: Option<String>,
    goal: Option<GoalView>,
    todos: Option<TodoList>,
}

impl RemotePort {
    pub fn absorb_replay(&mut self, replay: &Replay, entries: Vec<Entry>) {
        if replay.from == 0 {
            self.entries = entries;
        } else {
            self.entries.extend(entries);
        }
        self.leaf = replay.leaf.clone();
        self.goal = replay.goal.clone();
        if replay.todos.is_some() {
            self.todos = replay.todos.clone();
        }
    }

    pub fn set_goal(&mut self, goal: Option<GoalView>) {
        self.goal = goal;
    }

    pub fn set_todos(&mut self, todos: Option<TodoList>) {
        self.todos = todos;
    }
}

impl SessionPort for RemotePort {
    fn history(&mut self) -> Answer {
        Answer::Later
    }

    fn entries(&mut self) -> Answer {
        Answer::now(Reply::Entries {
            entries: self.entries.clone(),
            leaf: self.leaf.clone(),
        })
    }

    fn rewind(&mut self, entry_id: &str) -> Answer {
        self.queue.push(PortRequest::Rewind(entry_id.to_owned()));
        Answer::Later
    }

    fn new_session(&mut self, _session_dir: &str, _cwd: &str) -> Answer {
        self.queue.push(PortRequest::New);
        Answer::Later
    }

    fn undo(&mut self, _cwd: &str) -> Answer {
        self.queue.push(PortRequest::Undo);
        Answer::Later
    }

    fn slash(&mut self, line: &str, _session_dir: &str, _cwd: &str) -> Answer {
        self.queue.push(PortRequest::Slash(line.to_owned()));
        Answer::Later
    }

    fn select(&mut self, model: Model, effort: Effort) -> Answer {
        self.queue
            .push(PortRequest::Select(Box::new(model), effort));
        Answer::Later
    }

    fn plan(&mut self) -> Answer {
        self.queue.push(PortRequest::Plan);
        Answer::Later
    }

    fn goal(&self) -> Option<GoalView> {
        self.goal.clone()
    }

    fn todo_list(&self) -> Option<TodoList> {
        self.todos.clone()
    }
}

/// A `_yi/replay` frame's envelope; the entries decode separately so a bad one is counted.
#[derive(Debug, Clone)]
pub struct Replay {
    pub from: u64,
    pub leaf: Option<String>,
    pub name: Option<String>,
    pub goal: Option<GoalView>,
    pub todos: Option<TodoList>,
    pub context_window: Option<u64>,
    pub child: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub model: Option<(String, String)>,
    pub effort: Option<Effort>,
    pub context_window: Option<u64>,
}

pub enum Decoded {
    Event {
        seq: EventSeq,
        child: Option<String>,
        event: Box<AgentEvent>,
    },
    Gap,
    Replay(Box<Replay>, Vec<Entry>),
    Goal(Option<GoalView>),
    Todo(Option<TodoList>),
    Config(Config),
    Child(ChildUpdate),
    Workdir {
        cwd: String,
        lane: Option<String>,
    },
    Notice(String),
    Other,
}

pub struct Malformed;

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

pub fn goal_view(value: &Value) -> Option<GoalView> {
    let objective = string(value.get("objective"))?;
    Some(GoalView {
        objective,
        status: string(value.get("status")).unwrap_or_default(),
        tokens_used: value
            .get("tokens_used")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        token_budget: value.get("token_budget").and_then(Value::as_u64),
    })
}

pub fn config_of(options: &[AcpConfigOption]) -> Config {
    let mut config = Config::default();
    for option in options {
        let value = option.kind.get("value").and_then(Value::as_str);
        match (option.config_id.as_str(), value) {
            ("model", Some(value)) => {
                config.model = value
                    .split_once('/')
                    .map(|(provider, id)| (provider.to_owned(), id.to_owned()));
            }
            ("thought_level", Some(value)) => config.effort = value.parse().ok(),
            _ => {}
        }
    }
    config
}

/// `session/update` params; a `_`-prefixed kind is an extension, moved into its fields
/// directly rather than through the untagged enum, which buffers and copies the whole frame.
pub fn update_params(mut params: Value) -> Option<AcpUpdateParams> {
    let kind = params
        .pointer("/update/sessionUpdate")
        .and_then(Value::as_str);
    if !kind.is_some_and(|kind| kind.starts_with('_')) {
        return serde_json::from_value(params).ok();
    }
    let id = params.get_mut("sessionId")?.take();
    let update = params.get_mut("update")?.take();
    let (Value::String(session_id), Value::Object(mut fields)) = (id, update) else {
        return None;
    };
    let Value::String(session_update) = fields.remove("sessionUpdate")? else {
        return None;
    };
    Some(AcpUpdateParams {
        session_id,
        update: AcpSessionUpdate::Extension(AcpExtensionUpdate {
            session_update,
            fields: fields.into_iter().collect(),
        }),
    })
}

pub fn decode(extension: AcpExtensionUpdate) -> Result<Decoded, Malformed> {
    let mut fields = extension.fields;
    match extension.session_update.as_str() {
        "_yi/event" => {
            let seq = fields.get("seq").and_then(Value::as_u64).ok_or(Malformed)?;
            let event = fields.remove("event").ok_or(Malformed)?;
            let event = serde_json::from_value::<AgentEvent>(event).map_err(|_| Malformed)?;
            Ok(Decoded::Event {
                seq: EventSeq(seq),
                child: string(fields.get("childId")),
                event: Box::new(event),
            })
        }
        "_yi/event_gap" => Ok(Decoded::Gap),
        "_yi/replay" => {
            let entries = fields.remove("entries").ok_or(Malformed)?;
            let entries = serde_json::from_value::<Vec<Entry>>(entries).map_err(|_| Malformed)?;
            let replay = Replay {
                from: fields.get("from").and_then(Value::as_u64).unwrap_or(0),
                leaf: string(fields.get("leafId")),
                name: string(fields.get("name")),
                goal: fields.get("goal").and_then(goal_view),
                todos: fields
                    .remove("todos")
                    .and_then(|value| serde_json::from_value::<TodoList>(value).ok()),
                context_window: fields.get("contextWindow").and_then(Value::as_u64),
                child: string(fields.get("childId")),
            };
            Ok(Decoded::Replay(Box::new(replay), entries))
        }
        "_yi/workdir" => Ok(Decoded::Workdir {
            cwd: string(fields.get("cwd")).ok_or(Malformed)?,
            lane: string(fields.get("lane")),
        }),
        "_yi/goal" => Ok(Decoded::Goal(fields.get("goal").and_then(goal_view))),
        "_yi/todo" => {
            Ok(Decoded::Todo(fields.remove("list").and_then(|value| {
                serde_json::from_value::<TodoList>(value).ok()
            })))
        }
        "_yi/config" => {
            let options = fields
                .remove("configOptions")
                .and_then(|value| serde_json::from_value::<Vec<AcpConfigOption>>(value).ok())
                .unwrap_or_default();
            let mut config = config_of(&options);
            config.context_window = fields.get("contextWindow").and_then(Value::as_u64);
            Ok(Decoded::Config(config))
        }
        "_yi/notice" => Ok(Decoded::Notice(
            string(fields.get("text")).ok_or(Malformed)?,
        )),
        "_yi/subagent_update" => {
            serde_json::from_value::<ChildUpdate>(Value::Object(fields.into_iter().collect()))
                .map(Decoded::Child)
                .map_err(|_| Malformed)
        }
        _ => Ok(Decoded::Other),
    }
}

/// Only a transcript write spends a replay offset; config, workdir, goal and todo notes do not.
pub fn writes_transcript(update: &AcpSessionUpdate) -> bool {
    match update {
        AcpSessionUpdate::StateUpdate(_) | AcpSessionUpdate::UsageUpdate { .. } => false,
        AcpSessionUpdate::Extension(extension) => !matches!(
            extension.session_update.as_str(),
            "_yi/config" | "_yi/workdir" | "_yi/goal" | "_yi/todo" | "_yi/notice"
        ),
        _ => true,
    }
}

/// The wire option the chat's answer names: its own ids, and the nth of an always-choice's kind.
pub fn option_for(choice: AskChoice, options: &[AcpPermissionOption]) -> String {
    let nth = |kind: AcpPermissionOptionKind, index: usize| {
        options
            .iter()
            .filter(|option| option.kind == kind)
            .nth(index)
            .map(|option| option.option_id.clone())
    };
    let wanted = match choice {
        AskChoice::AllowOnce => nth(AcpPermissionOptionKind::AllowOnce, 0),
        AskChoice::AllowAlways(index) => nth(AcpPermissionOptionKind::AllowAlways, index)
            .or_else(|| nth(AcpPermissionOptionKind::AllowAlways, 0))
            .or_else(|| nth(AcpPermissionOptionKind::AllowOnce, 0)),
        AskChoice::Reject => nth(AcpPermissionOptionKind::RejectOnce, 0),
    };
    wanted.unwrap_or_else(|| match choice {
        AskChoice::AllowOnce | AskChoice::AllowAlways(_) => "allow_once".to_owned(),
        AskChoice::Reject => "reject_once".to_owned(),
    })
}

pub fn grant_labels(options: &[AcpPermissionOption]) -> Vec<String> {
    options
        .iter()
        .filter(|option| option.kind == AcpPermissionOptionKind::AllowAlways)
        .filter_map(|option| option.name.strip_prefix("Always allow ").map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod todo_tests {
    use super::*;
    use yi_types::acp::AcpExtensionUpdate;

    #[test]
    fn a_todo_update_decodes_to_the_list_and_a_null_clears_it() -> Result<(), String> {
        let list = serde_json::json!({"phases": [{"name": "Tasks", "items": [{"label": "a", "state": "running"}]}]});
        let update = AcpExtensionUpdate {
            session_update: "_yi/todo".to_owned(),
            fields: std::iter::once(("list".to_owned(), list)).collect(),
        };
        match decode(update).map_err(|_| "malformed".to_owned())? {
            Decoded::Todo(Some(list)) => assert_eq!(list.progress().open, 1),
            _ => return Err("not a todo update".to_owned()),
        }
        let cleared = AcpExtensionUpdate {
            session_update: "_yi/todo".to_owned(),
            fields: std::iter::once(("list".to_owned(), Value::Null)).collect(),
        };
        assert!(matches!(
            decode(cleared).map_err(|_| "malformed".to_owned())?,
            Decoded::Todo(None)
        ));
        Ok(())
    }

    #[test]
    fn extension_params_move_into_fields_and_standard_kinds_still_parse() -> Result<(), String> {
        use serde_json::json;
        let replay = json!({"sessionId": "s1", "update": {
            "sessionUpdate": "_yi/replay", "entries": [], "from": 3, "leafId": "e9",
        }});
        let params = update_params(replay).ok_or("replay params")?;
        assert_eq!(params.session_id, "s1");
        let AcpSessionUpdate::Extension(extension) = params.update else {
            return Err("not an extension".to_owned());
        };
        match decode(extension).map_err(|_| "malformed".to_owned())? {
            Decoded::Replay(replay, entries) => {
                assert_eq!((replay.from, replay.leaf.as_deref()), (3, Some("e9")));
                assert!(entries.is_empty());
            }
            _ => return Err("not a replay".to_owned()),
        }
        let state = json!({"sessionId": "s1", "update": {"sessionUpdate": "state_update", "state": "running"}});
        let params = update_params(state).ok_or("state params")?;
        assert!(matches!(params.update, AcpSessionUpdate::StateUpdate(_)));
        assert!(
            update_params(json!({"sessionId": "s1", "update": {"sessionUpdate": "_yi/x"}}))
                .is_some()
        );
        assert!(update_params(json!({"update": {"sessionUpdate": "_yi/x"}})).is_none());
        Ok(())
    }
}

#[cfg(test)]
mod ask_tests {
    use super::*;
    use yi_types::acp::AcpExtensionUpdate;

    fn option(id: &str, name: &str, kind: AcpPermissionOptionKind) -> AcpPermissionOption {
        AcpPermissionOption {
            option_id: id.to_owned(),
            name: name.to_owned(),
            kind,
        }
    }

    #[test]
    fn a_workdir_update_decodes_path_and_lane() {
        let update = AcpExtensionUpdate {
            session_update: "_yi/workdir".to_owned(),
            fields: [
                (
                    "cwd".to_owned(),
                    serde_json::json!("/home/user/.yi/lanes/abc/1"),
                ),
                ("lane".to_owned(), serde_json::json!("project ⎇ lane 1")),
            ]
            .into_iter()
            .collect(),
        };
        let decoded = decode(update);
        let named = matches!(
            decoded,
            Ok(Decoded::Workdir { ref cwd, ref lane })
                if cwd == "/home/user/.yi/lanes/abc/1"
                    && lane.as_deref() == Some("project ⎇ lane 1")
        );
        assert!(named, "the update carries the lane path and its row label");
    }

    #[test]
    fn option_for_picks_the_nth_always_option_and_labels_it() {
        let options = vec![
            option(
                "allow_once",
                "Allow once",
                AcpPermissionOptionKind::AllowOnce,
            ),
            option(
                "allow_always",
                "Always allow edits under crates/tui/src",
                AcpPermissionOptionKind::AllowAlways,
            ),
            option(
                "allow_always_1",
                "Always allow edits anywhere in this tree",
                AcpPermissionOptionKind::AllowAlways,
            ),
            option("reject_once", "Reject", AcpPermissionOptionKind::RejectOnce),
        ];
        assert_eq!(
            option_for(AskChoice::AllowAlways(1), &options),
            "allow_always_1"
        );
        assert_eq!(
            option_for(AskChoice::AllowAlways(0), &options),
            "allow_always"
        );
        assert_eq!(
            option_for(AskChoice::AllowAlways(9), &options),
            "allow_always",
            "a grant the worker never offered falls back to the narrowest one"
        );
        assert_eq!(option_for(AskChoice::Reject, &options), "reject_once");
        assert_eq!(
            grant_labels(&options),
            ["edits under crates/tui/src", "edits anywhere in this tree"]
        );
    }
}
