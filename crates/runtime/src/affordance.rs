use std::sync::OnceLock;

use serde_json::{Map, Value};
use yi_types::graph::{Edge, Graph};

/// Host-authored next steps: deterministic, at most two lines, and immutable
/// once written, because a recomputed line would move transcript bytes.
pub const NEXT: &str = "next: ";

/// A tool result carries at most this many lines; the todo tool keeps its own three.
pub const TOOL_LINES: usize = 2;
const HOPS: usize = 2;

/// What the host knows at the seam: the predicates that hold, and the one name a line cites.
pub struct Facts<'a> {
    pub holds: &'a [&'a str],
    pub name: &'a str,
    pub cap: usize,
}

/// The graph this binary carries, parsed once; an unparsable graph renders nothing, and
/// `the_shipped_graph_passes_every_structural_check` is what keeps that from shipping.
pub fn shipped() -> &'static Graph {
    static GRAPH: OnceLock<Graph> = OnceLock::new();
    GRAPH.get_or_init(|| {
        serde_json::from_str(include_str!("prompts/graph.json")).unwrap_or_default()
    })
}

/// Invariant: a pure function of the graph and the facts, localized by exact match on the
/// last call, so a line written once never moves transcript bytes.
pub fn render(graph: &Graph, last_call: &str, facts: &Facts<'_>) -> Vec<String> {
    let holds = |edge: &&Edge| {
        let condition = edge.condition.as_str();
        condition == "always" || facts.holds.contains(&condition)
    };
    let mut edges: Vec<&Edge> = Vec::new();
    let mut frontier = vec![last_call];
    for _ in 0..HOPS {
        let found: Vec<&Edge> = frontier
            .iter()
            .flat_map(|from| graph.out_edges(from))
            .filter(holds)
            .collect();
        frontier = found.iter().map(|edge| edge.to.as_str()).collect();
        edges.extend(found);
    }
    edges.sort_by_key(|edge| std::cmp::Reverse(edge.weight));
    let mut lines: Vec<String> = Vec::new();
    for edge in edges {
        let line = format!("{NEXT}{}", edge.guidance.replace("{name}", facts.name));
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines.truncate(facts.cap);
    lines
}

/// The shipped graph's lines after `last_call`, joined for a notice or a reply field.
pub fn next(last_call: &str, holds: &[&str], name: &str) -> String {
    let facts = Facts {
        holds,
        name,
        cap: TOOL_LINES,
    };
    render(shipped(), last_call, &facts).join("\n")
}

pub fn call_template(tool: &str, schema: &Value, arguments: &Map<String, Value>) -> String {
    let properties = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut shape = Map::new();
    if let Some(properties) = properties {
        for (key, spec) in properties {
            let wanted = required.contains(&key.as_str()) || arguments.contains_key(key);
            if !wanted {
                continue;
            }
            let value = arguments.get(key).cloned().unwrap_or_else(|| {
                Value::String(format!(
                    "<{}>",
                    spec.get("type").and_then(Value::as_str).unwrap_or("value")
                ))
            });
            shape.insert(key.clone(), value);
        }
    }
    // Incident: the plan schema names no `attempt`, so a submit template printed without it and
    // the parser refused the very call the surface had just shown (#478).
    for (key, value) in arguments {
        shape.entry(key.clone()).or_insert_with(|| value.clone());
    }
    let rendered = serde_json::to_string(&Value::Object(shape)).unwrap_or_else(|_| "{}".to_owned());
    format!("{NEXT}call {tool} as {rendered}")
}

pub fn append(result: &mut yi_types::event::ToolResult, line: &str) {
    use yi_types::message::Content;
    match result.content.last_mut() {
        Some(Content::Text { text, .. }) => {
            text.push('\n');
            text.push_str(line);
        }
        _ => result.content.push(Content::Text {
            text: line.to_owned(),
            text_signature: None,
        }),
    }
}
