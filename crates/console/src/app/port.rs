//! The remote port: the chat's session touches become daemon requests, and the `_yi/*`
//! updates the daemon streams decode into the chat's own events.

use serde_json::Value;
use yi_tui::hud::GoalView;
use yi_tui::{Answer, AskChoice, Reply, SessionPort};
use yi_types::acp::{
    AcpConfigOption, AcpExtensionUpdate, AcpPermissionOption, AcpPermissionOptionKind,
};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::model::{Effort, Model};
use yi_types::subagent::ChildUpdate;

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
    }

    pub fn set_goal(&mut self, goal: Option<GoalView>) {
        self.goal = goal;
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
}

/// A `_yi/replay` frame's envelope; the entries decode separately so a bad one is counted.
#[derive(Debug, Clone)]
pub struct Replay {
    pub from: u64,
    pub leaf: Option<String>,
    pub name: Option<String>,
    pub goal: Option<GoalView>,
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
    Config(Config),
    Child(ChildUpdate),
    Other,
}

pub struct Malformed;

fn string(fields: &Value, key: &str) -> Option<String> {
    fields.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub fn goal_view(value: &Value) -> Option<GoalView> {
    let objective = string(value, "objective")?;
    Some(GoalView {
        objective,
        status: string(value, "status").unwrap_or_default(),
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

pub fn decode(extension: &AcpExtensionUpdate) -> Result<Decoded, Malformed> {
    let fields = Value::Object(
        extension
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    match extension.session_update.as_str() {
        "_yi/event" => {
            let seq = fields.get("seq").and_then(Value::as_u64).ok_or(Malformed)?;
            let event = fields.get("event").cloned().ok_or(Malformed)?;
            let event = serde_json::from_value::<AgentEvent>(event).map_err(|_| Malformed)?;
            Ok(Decoded::Event {
                seq: EventSeq(seq),
                child: string(&fields, "childId"),
                event: Box::new(event),
            })
        }
        "_yi/event_gap" => Ok(Decoded::Gap),
        "_yi/replay" => {
            let entries = fields.get("entries").cloned().ok_or(Malformed)?;
            let entries = serde_json::from_value::<Vec<Entry>>(entries).map_err(|_| Malformed)?;
            let replay = Replay {
                from: fields.get("from").and_then(Value::as_u64).unwrap_or(0),
                leaf: string(&fields, "leafId"),
                name: string(&fields, "name"),
                goal: fields.get("goal").and_then(goal_view),
                context_window: fields.get("contextWindow").and_then(Value::as_u64),
                child: string(&fields, "childId"),
            };
            Ok(Decoded::Replay(Box::new(replay), entries))
        }
        "_yi/goal" => Ok(Decoded::Goal(fields.get("goal").and_then(goal_view))),
        "_yi/config" => {
            let options = fields.get("configOptions").cloned().unwrap_or(Value::Null);
            let options =
                serde_json::from_value::<Vec<AcpConfigOption>>(options).unwrap_or_default();
            let mut config = config_of(&options);
            config.context_window = fields.get("contextWindow").and_then(Value::as_u64);
            Ok(Decoded::Config(config))
        }
        "_yi/subagent_update" => serde_json::from_value::<ChildUpdate>(fields)
            .map(Decoded::Child)
            .map_err(|_| Malformed),
        _ => Ok(Decoded::Other),
    }
}

/// The wire option the chat's answer names; the request's own ids, never invented ones.
pub fn option_for(choice: AskChoice, options: &[AcpPermissionOption]) -> String {
    let wanted = match choice {
        AskChoice::AllowOnce => AcpPermissionOptionKind::AllowOnce,
        AskChoice::AllowAlways => AcpPermissionOptionKind::AllowAlways,
        AskChoice::Reject => AcpPermissionOptionKind::RejectOnce,
    };
    let by_kind = |kind: AcpPermissionOptionKind| {
        options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| option.option_id.clone())
    };
    by_kind(wanted)
        .or_else(|| {
            (choice == AskChoice::AllowAlways)
                .then(|| by_kind(AcpPermissionOptionKind::AllowOnce))
                .flatten()
        })
        .unwrap_or_else(|| match choice {
            AskChoice::AllowOnce | AskChoice::AllowAlways => "allow_once".to_owned(),
            AskChoice::Reject => "reject_once".to_owned(),
        })
}
