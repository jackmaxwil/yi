use serde_json::Value;
use yi_types::plan::contract::Decider;
use yi_types::plan::doc::{PlanId, Todo};
use yi_types::plan::ledger::JournalRecord;

use super::artifact::Artifacts;

pub(super) const KEY: &str = "brief_lines";
const SCHEMA_CHARS: usize = 2_000;

pub(super) fn lines(
    artifacts: &Artifacts,
    records: &[JournalRecord],
    id: &PlanId,
    todo: &Todo,
) -> Vec<String> {
    let mut lines = Vec::new();
    let schemas: Vec<_> = todo
        .contract
        .iter()
        .flat_map(|contract| &contract.items)
        .filter_map(|item| match &item.decider {
            Decider::Schema { schema } => Some(schema),
            _ => None,
        })
        .collect();
    let declared = todo
        .delegation
        .as_ref()
        .is_some_and(|delegation| delegation.output.is_some());
    if declared || !schemas.is_empty() {
        lines.push(
            "Answer with only a JSON value matching the schema: no prose and no fence around it."
                .to_owned(),
        );
    }
    for schema in schemas {
        let text = artifacts
            .get(&schema.digest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map(|value| value.to_string())
            .filter(|text| text.chars().count() <= SCHEMA_CHARS);
        lines.push(match text {
            Some(text) => format!("Schema: {text}"),
            None => format!("Schema: read plan://{id}/artifacts/{}", schema.digest.hex()),
        });
    }
    let failed = records.iter().rev().find(|record| {
        record.record.plan == *id
            && record.record.todo.as_ref() == Some(&todo.label)
            && record.record.op == "fail"
            && record
                .attempt
                .is_some_and(|attempt| attempt.get().saturating_add(1) == todo.attempt.get())
    });
    if let Some(record) = failed {
        let cause = record.args.get("cause").and_then(Value::as_str);
        let trace = record
            .record
            .extra
            .get("reaped")
            .and_then(|reaped| reaped.get(0)?.get("last")?.as_str());
        let mut line = format!(
            "Your previous attempt failed: {}.",
            cause.unwrap_or("no cause was given")
        );
        if let Some(trace) = trace {
            line.push_str(&format!(" Its transcript is {trace}."));
        }
        lines.push(line);
    }
    lines
}
