//! Child agents, one level deep, as the server reports them.

use yi_types::subagent::{
    ChildActivity, ChildExit, ChildFlag, ChildId, ChildStatus, ChildTrail, ChildUpdate,
};

use crate::ids::{AgentId, Millis};
use crate::reduce::turns::{CallId, Transcript};

/// Child agents one thread keeps; past it a new one is not added.
pub const MAX_AGENTS: usize = 1024;

/// One agent a thread started.
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub id: AgentId,
    /// The agent whose stream reported it; none when the thread itself did.
    pub parent: Option<AgentId>,
    /// Its status, activity, counts, cost, flag, role and task, as the server last said.
    pub update: ChildUpdate,
    /// When this app first and last heard of it.
    pub first_seen: Millis,
    pub last_seen: Millis,
    /// Since when it has waited on the user or been stuck.
    pub flagged: Option<Millis>,
    /// What it said and did, from its own events.
    pub transcript: Transcript,
    /// The parent's step that started it, once the parent's trail names it.
    pub spawned_by: Option<CallId>,
    /// The server said it has no such child when its conversation was asked for.
    pub gone: bool,
}

/// What a live child is doing, as the server last said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Waiting,
    Writing,
    Executing,
}

/// Why a child is flagged; `Other` is a state a newer server named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    NeedsYou,
    Stuck,
    Other,
}

impl Agent {
    pub fn doing(&self) -> Doing {
        match self.update.activity {
            ChildActivity::Waiting => Doing::Waiting,
            ChildActivity::Writing => Doing::Writing,
            ChildActivity::Executing => Doing::Executing,
        }
    }

    /// How its run ended, in a word; none while it runs.
    pub fn ended(&self) -> Option<&'static str> {
        let exit = match &self.update.exit {
            Some(exit) => exit,
            None if self.update.status == ChildStatus::Running => return None,
            None => return Some("ended"),
        };
        Some(match exit {
            ChildExit::Completed => "finished",
            ChildExit::Failed { .. } => "failed",
            ChildExit::Interrupted => "stopped",
            ChildExit::Reaped => "reaped",
            ChildExit::Repossessed => "taken back",
            ChildExit::Other => "ended",
        })
    }

    /// The question it waits on, as the server notes it (`asks <id>: <words>`): id and words.
    pub fn question(&self) -> Option<(&str, &str)> {
        match &self.update.flag {
            Some(ChildFlag::NeedsYou { note }) => note.strip_prefix("asks ")?.split_once(": "),
            Some(ChildFlag::Stuck { .. } | ChildFlag::Other) | None => None,
        }
    }

    pub fn flag(&self) -> Option<Flag> {
        self.update.flag.as_ref().map(|flag| match flag {
            ChildFlag::NeedsYou { .. } => Flag::NeedsYou,
            ChildFlag::Stuck { .. } => Flag::Stuck,
            ChildFlag::Other => Flag::Other,
        })
    }
}

/// What is known of a child before the server has said anything about it but its name.
fn unreported(id: &AgentId, name: &str) -> ChildUpdate {
    ChildUpdate {
        id: ChildId(id.as_str().to_owned()),
        name: name.to_owned(),
        status: ChildStatus::Running,
        activity: ChildActivity::Waiting,
        tool_use_count: 0,
        token_count: 0,
        cost: None,
        answer_preview: None,
        error: None,
        exit: None,
        flag: None,
        session: None,
        role: None,
        task: None,
        elapsed_ms: None,
    }
}

/// The agent `id` of `agents`, added if there is room and it is new.
pub(crate) fn agent<'a>(
    agents: &'a mut Vec<Agent>,
    id: &AgentId,
    at: Millis,
) -> Option<&'a mut Agent> {
    let found = agents.iter().position(|agent| &agent.id == id);
    let index = match found {
        Some(index) => index,
        None if agents.len() >= MAX_AGENTS => return None,
        None => {
            agents.push(Agent {
                id: id.clone(),
                parent: None,
                update: unreported(id, id.as_str()),
                first_seen: at,
                last_seen: at,
                flagged: None,
                transcript: Transcript::default(),
                spawned_by: None,
                gone: false,
            });
            agents.len().saturating_sub(1)
        }
    };
    agents.get_mut(index)
}

/// Takes the server's latest word on a child; `parent` is the child whose stream said it.
pub(crate) fn report(
    agents: &mut Vec<Agent>,
    update: ChildUpdate,
    parent: Option<AgentId>,
    at: Millis,
) {
    let id = AgentId::new(update.id.as_str());
    let Some(agent) = agent(agents, &id, at) else {
        return;
    };
    agent.flagged = match (&update.flag, agent.flagged) {
        (Some(_), since) => since.or(Some(at)),
        (None, _) => None,
    };
    agent.last_seen = at;
    agent.parent = parent.or(agent.parent.take());
    agent.update = update;
}

/// Takes a line of the trail a thread keeps of its children: what a replay knows of the
/// ones that ended before this app was looking.
pub(crate) fn trail(agents: &mut Vec<Agent>, trail: ChildTrail, at: Millis) {
    match trail {
        ChildTrail::Spawned(spawned) => {
            let id = AgentId::new(spawned.id.as_str());
            let Some(agent) = agent(agents, &id, at) else {
                return;
            };
            if agent.update.name == id.as_str() {
                agent.update.name = spawned.name;
            }
            agent.update.role = agent.update.role.take().or(spawned.role);
            agent.update.task = agent.update.task.take().or(spawned.task);
            agent.update.session = agent.update.session.take().or(Some(spawned.session));
            agent.spawned_by = spawned.spawned_by.map(CallId::new);
        }
        ChildTrail::Ended(ended) => {
            let id = AgentId::new(ended.id.as_str());
            let Some(agent) = agent(agents, &id, at) else {
                return;
            };
            // A child the server still reports on is newer than its trail.
            if agent.update.exit.is_some() {
                return;
            }
            agent.update.status = match ended.exit {
                ChildExit::Completed => ChildStatus::Completed,
                ChildExit::Failed { .. }
                | ChildExit::Interrupted
                | ChildExit::Reaped
                | ChildExit::Repossessed
                | ChildExit::Other => ChildStatus::Error,
            };
            agent.update.exit = Some(ended.exit);
            agent.update.error = ended.error;
            agent.update.token_count = ended.tokens;
            agent.update.elapsed_ms = ended.elapsed_ms;
            agent.update.cost = ended.cost;
            agent.update.answer_preview = ended.answer_preview;
            agent.last_seen = agent.last_seen.max(at);
        }
        ChildTrail::Other => {}
    }
}
