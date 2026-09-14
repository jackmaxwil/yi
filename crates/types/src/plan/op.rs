//! The plan op as a wire shape (plan section 5.3): the tag is the journal's `op` and the
//! fields are its `args`, so the same type parses a tool call and replays a journal record.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::doc::{AgentId, BlockedOn, Delegation, GoalText, Todo, TodoLabel, TodoStateName};
use super::ledger::{AttemptId, EffectId};
use crate::url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    Init,
    Append,
    Drop,
    Block,
    Unblock,
    Reorder,
    AddEdge,
    Start,
    Done,
    Fail,
    Retry,
    Decompose,
    Supersede,
    Set,
    View,
    FuseReset,
    Repair,
    Import,
    Reconcile,
    Submit,
    Resolve,
    Accept,
}

/// The kinds a model may call, in the tool schema's order; the administrative kinds below
/// parse on every surface but cost the prompt no bytes.
pub const MODEL_OPS: usize = 15;

/// Every kind, in wire order; the parser reads this list and the schema its first `MODEL_OPS`.
pub const ALL_OPS: [OpKind; 22] = [
    OpKind::Set,
    OpKind::Init,
    OpKind::Append,
    OpKind::Drop,
    OpKind::Block,
    OpKind::Unblock,
    OpKind::Reorder,
    OpKind::AddEdge,
    OpKind::Start,
    OpKind::Done,
    OpKind::Fail,
    OpKind::Retry,
    OpKind::Decompose,
    OpKind::Supersede,
    OpKind::View,
    OpKind::FuseReset,
    OpKind::Repair,
    OpKind::Import,
    OpKind::Reconcile,
    OpKind::Submit,
    OpKind::Resolve,
    OpKind::Accept,
];

pub fn op_name(op: OpKind) -> &'static str {
    match op {
        OpKind::Init => "init",
        OpKind::Append => "append",
        OpKind::Drop => "drop",
        OpKind::Block => "block",
        OpKind::Unblock => "unblock",
        OpKind::Reorder => "reorder",
        OpKind::AddEdge => "add_edge",
        OpKind::Start => "start",
        OpKind::Done => "done",
        OpKind::Fail => "fail",
        OpKind::Retry => "retry",
        OpKind::Decompose => "decompose",
        OpKind::Supersede => "supersede",
        OpKind::Set => "set",
        OpKind::View => "view",
        OpKind::FuseReset => "fuse_reset",
        OpKind::Repair => "repair",
        OpKind::Import => "import",
        OpKind::Reconcile => "reconcile",
        OpKind::Submit => "submit",
        OpKind::Resolve => "resolve",
        OpKind::Accept => "accepted_by_user",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoSpec {
    pub label: TodoLabel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<TodoLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<Delegation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<super::contract::Contract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Todo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetRow {
    pub spec: TodoSpec,
    pub state: TodoStateName,
}

/// A repair resolution for one todo that needs reconciliation: retry as a new attempt, or fail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub label: TodoLabel,
    pub action: Resolve,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolve {
    Retry,
    Fail { cause: String },
}

/// The host's own record of an effect it settled: a child reattached, or a durable result
/// reused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconciliation {
    Reattached { agent: AgentId },
    Reused { output: Url },
}

/// The op is its own wire shape: the tag is the journal's `op` and the fields are `args`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Init {
        goal: GoalText,
        todos: Vec<TodoSpec>,
    },
    Append {
        todos: Vec<TodoSpec>,
    },
    Drop {
        label: TodoLabel,
    },
    Block {
        label: TodoLabel,
        on: BlockedOn,
        note: String,
    },
    Unblock {
        label: TodoLabel,
    },
    Reorder {
        labels: Vec<TodoLabel>,
    },
    AddEdge {
        todo: TodoLabel,
        after: TodoLabel,
    },
    Start {
        label: TodoLabel,
    },
    Done {
        label: TodoLabel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Url>,
    },
    Fail {
        label: TodoLabel,
        cause: String,
    },
    Retry {
        label: TodoLabel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delegation: Option<Box<Delegation>>,
    },
    Decompose {
        label: TodoLabel,
        todos: Vec<TodoSpec>,
    },
    Supersede {
        reason: String,
        todos: Vec<TodoSpec>,
    },
    Set {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<GoalText>,
        rows: Vec<SetRow>,
    },
    View {
        full: bool,
    },
    FuseReset,
    Repair {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resolutions: Vec<Resolution>,
    },
    Import {
        source: Url,
    },
    Reconcile {
        label: TodoLabel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect_id: Option<EffectId>,
        outcome: Reconciliation,
    },
    /// The running agent's output for its own attempt (plan section 3.6); a submit for
    /// another agent's todo or a stale attempt is refused.
    Submit {
        label: TodoLabel,
        attempt: AttemptId,
        output: Url,
    },
    /// One todo's repair resolution, bound to the attempt the user saw (plan section 5.6).
    Resolve {
        label: TodoLabel,
        attempt: AttemptId,
        resolution: Resolve,
    },
    /// The user's administrative acceptance: `Done { AcceptedByUser }`, never verified.
    #[serde(rename = "accepted_by_user")]
    Accept {
        label: TodoLabel,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Url>,
    },
}

impl Op {
    pub fn label(&self) -> Option<&TodoLabel> {
        match self {
            Self::Drop { label }
            | Self::Block { label, .. }
            | Self::Unblock { label }
            | Self::Start { label }
            | Self::Done { label, .. }
            | Self::Fail { label, .. }
            | Self::Retry { label, .. }
            | Self::Decompose { label, .. }
            | Self::Reconcile { label, .. }
            | Self::Submit { label, .. }
            | Self::Resolve { label, .. }
            | Self::Accept { label, .. } => Some(label),
            Self::AddEdge { todo, .. } => Some(todo),
            Self::Init { .. }
            | Self::Append { .. }
            | Self::Reorder { .. }
            | Self::Supersede { .. }
            | Self::Set { .. }
            | Self::View { .. }
            | Self::FuseReset
            | Self::Repair { .. }
            | Self::Import { .. } => None,
        }
    }

    pub fn kind(&self) -> OpKind {
        match self {
            Self::Init { .. } => OpKind::Init,
            Self::Append { .. } => OpKind::Append,
            Self::Drop { .. } => OpKind::Drop,
            Self::Block { .. } => OpKind::Block,
            Self::Unblock { .. } => OpKind::Unblock,
            Self::Reorder { .. } => OpKind::Reorder,
            Self::AddEdge { .. } => OpKind::AddEdge,
            Self::Start { .. } => OpKind::Start,
            Self::Done { .. } => OpKind::Done,
            Self::Fail { .. } => OpKind::Fail,
            Self::Retry { .. } => OpKind::Retry,
            Self::Decompose { .. } => OpKind::Decompose,
            Self::Supersede { .. } => OpKind::Supersede,
            Self::Set { .. } => OpKind::Set,
            Self::View { .. } => OpKind::View,
            Self::FuseReset => OpKind::FuseReset,
            Self::Repair { .. } => OpKind::Repair,
            Self::Import { .. } => OpKind::Import,
            Self::Reconcile { .. } => OpKind::Reconcile,
            Self::Submit { .. } => OpKind::Submit,
            Self::Resolve { .. } => OpKind::Resolve,
            Self::Accept { .. } => OpKind::Accept,
        }
    }

    /// The journal's `args`: the op's fields without the tag.
    pub fn args(&self) -> Result<Value, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let Value::Object(map) = &mut value {
            map.remove("op");
        }
        Ok(value)
    }
}

/// A child reaped as a todo left Running, as the record carries it: the reducer reads `last`
/// from here instead of asking anyone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaped {
    pub plan: super::doc::PlanId,
    pub todo: TodoLabel,
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Url>,
}
