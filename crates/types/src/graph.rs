//! The procedural graph: Yi's verbs as nodes, host-authored guidance as typed edges (D219).
//! It is data the binary carries; a condition is a name from a closed set, never an expression.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const MAX_EDGES: usize = 400;
pub const MAX_OUT_EDGES: usize = 8;
pub const MAX_GUIDANCE_BYTES: usize = 160;
pub const MAX_PITFALLS: usize = 3;
pub const MAX_PITFALL_BYTES: usize = 120;

/// The closed predicate set: a name, and the space-separated arguments it takes one of.
/// Invariant: the host asserts these as facts at a seam; no model evaluates one.
pub const PREDICATES: &[(&str, &str)] = &[
    ("always", ""),
    ("result_ok", ""),
    (
        "result_error",
        "denied not_found invalid_args aborted stale_tag noop_loop tool_error",
    ),
    ("output_capped", ""),
    ("todo_open", ""),
    ("todo_state", "running pending blocked"),
    ("plan_ready_nonempty", ""),
    (
        "child_state",
        "queued running finished failed needs_you stuck repossession_pending",
    ),
    ("blocked_on", "user child external"),
    ("worktree_unmerged", ""),
    ("done_refused", ""),
    ("inbox_nonempty", ""),
    ("coroutine_unawaited", ""),
    ("method_awaited", ""),
    ("listing_name_missed", ""),
    ("grid_answer_empty", ""),
    ("session_on_disk", ""),
    ("session_in_memory", ""),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Predicate(String);

impl Predicate {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Predicate {
    type Error = String;

    fn try_from(raw: String) -> Result<Self, String> {
        let (name, argument) = match raw.strip_suffix(')').and_then(|open| open.split_once('(')) {
            Some((name, argument)) => (name, Some(argument)),
            None => (raw.as_str(), None),
        };
        let known = PREDICATES.iter().any(|(known, arguments)| {
            *known == name
                && match argument {
                    Some(argument) => arguments.split_whitespace().any(|known| known == argument),
                    None => arguments.is_empty(),
                }
        });
        if known {
            Ok(Self(raw))
        } else {
            Err(format!("unknown predicate {raw:?}"))
        }
    }
}

impl From<Predicate> for String {
    fn from(predicate: Predicate) -> Self {
        predicate.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Tool,
    Op,
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Then,
    Instead,
    Before,
    AfterError,
    AfterRefusal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub relation: Relation,
    pub to: String,
    pub condition: Predicate,
    pub guidance: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitfalls: Vec<String>,
    pub weight: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub version: u32,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn out_edges<'a>(&'a self, from: &'a str) -> impl Iterator<Item = &'a Edge> {
        self.edges.iter().filter(move |edge| edge.from == from)
    }

    /// The structural checks, in order; the error is the failed check's name, which the
    /// offline refiner records as a rejection's reason.
    pub fn check(&self, verbs: &HashSet<&str>) -> Result<(), &'static str> {
        let nodes: HashSet<&str> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        if !nodes.iter().all(|id| verbs.contains(id)) {
            return Err("unregistered_node");
        }
        if self.edges.len() > MAX_EDGES {
            return Err("too_many_edges");
        }
        let mut reached: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Tool)
            .map(|node| node.id.as_str())
            .collect();
        while let Some(edge) = self.edges.iter().find(|edge| {
            reached.contains(edge.from.as_str()) && !reached.contains(edge.to.as_str())
        }) {
            reached.insert(&edge.to);
        }
        for edge in &self.edges {
            if !nodes.contains(edge.from.as_str()) || !nodes.contains(edge.to.as_str()) {
                return Err("unknown_node");
            }
            if edge.from == edge.to {
                return Err("self_edge");
            }
            if self.out_edges(&edge.from).count() > MAX_OUT_EDGES {
                return Err("too_many_out_edges");
            }
            if edge.guidance.is_empty() || edge.guidance.len() > MAX_GUIDANCE_BYTES {
                return Err("guidance_size");
            }
            let pitfalls_fit = edge
                .pitfalls
                .iter()
                .all(|pitfall| pitfall.len() <= MAX_PITFALL_BYTES);
            if edge.pitfalls.len() > MAX_PITFALLS || !pitfalls_fit {
                return Err("pitfalls_size");
            }
            if !reached.contains(edge.to.as_str()) {
                return Err("unreachable");
            }
        }
        Ok(())
    }
}
