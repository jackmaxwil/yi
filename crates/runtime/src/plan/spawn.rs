use std::path::Path;

use serde_json::{Map, Value};
use yi_types::plan::canonical::DIGEST_PREFIX;
use yi_types::plan::contract::Decider;
use yi_types::plan::doc::{AgentId, Check, Delegation, Isolation, Todo, TodoAddr};
use yi_types::url::Scheme;

use super::artifact::Artifacts;
use super::output::OutputResolve;

/// The routed reader's answer schema, carried from the spawn's start like the brief's lines.
pub(super) const SCHEMA: &str = "reader_schema";

/// A child is addressed by the todo it executes — its name is the todo
/// address's URL path, so the live and reaped trace addresses need no map.
pub(super) fn child_name(at: &TodoAddr) -> Result<AgentId, String> {
    let address = at.to_url().map_err(|error| error.to_string())?;
    AgentId::new(address.path()).map_err(|error| error.to_string())
}

fn accept_line(accept: &Check) -> String {
    match accept {
        Check::Command(command) => format!("Acceptance: exit 0 of `{command}`"),
        Check::Stated(stated) => format!("Acceptance: {stated}"),
        Check::Other(other) => format!("Acceptance: {other}"),
    }
}

/// A Reader runs as a reader child only when nothing it was given needs a root: no check, no
/// isolation, read and grep alone, and a partition of files, which is all a reader can inline.
pub(super) fn reads_only(delegation: &Delegation, cwd: &Path) -> bool {
    let spec = &delegation.spec;
    spec.role.as_deref() == Some("reader")
        && !matches!(delegation.accept, Check::Command(_))
        && matches!(spec.isolation, None | Some(Isolation::None))
        && spec
            .tools
            .iter()
            .all(|tool| ["read", "grep"].contains(&tool.as_str()))
        && delegation.context.iter().all(|url| match url.scheme() {
            Scheme::Local => !cwd.join(url.path()).is_dir(),
            scheme => !matches!(scheme.as_str(), "kernel" | "family"),
        })
}

/// The declared output's schema, else the contract's schema item, as the JSON object it holds.
pub(super) fn schema(
    artifacts: &Artifacts,
    todo: &Todo,
    resolve: Option<&dyn OutputResolve>,
) -> Option<Value> {
    let output = todo.delegation.as_ref().and_then(|it| it.output.as_ref());
    let text = match output {
        Some(output) => resolve?.resolve(&output.schema).ok()??,
        None => {
            let digest = todo
                .contract
                .iter()
                .flat_map(|contract| &contract.items)
                .find_map(|item| match &item.decider {
                    Decider::Schema { schema } => Some(&schema.digest),
                    _ => None,
                })?;
            String::from_utf8(artifacts.get(digest).ok()?).ok()?
        }
    };
    serde_json::from_str(&text).ok().filter(Value::is_object)
}

pub(super) fn brief(at: &TodoAddr, delegation: &Delegation, reader: bool) -> String {
    let mut lines = vec![format!(
        "Execute todo {:?} of plan {}.",
        at.todo.as_str(),
        at.plan
    )];
    if let Some(role) = &delegation.spec.role {
        lines.push(format!("Role: {role}"));
    }
    lines.push(accept_line(&delegation.accept));
    if let Some(output) = &delegation.output {
        lines.push(format!(
            "Answer with JSON matching the schema at {}",
            output.schema
        ));
    }
    if !delegation.context.is_empty() && !reader {
        lines.push("Context:".to_owned());
        for url in &delegation.context {
            lines.push(format!("- {url}"));
        }
    }
    if !delegation.spec.tools.is_empty() {
        lines.push(format!(
            "Prefer tools: {}",
            delegation.spec.tools.join(", ")
        ));
    }
    if let Some(budget) = &delegation.spec.budget {
        lines.push(format!(
            "Token budget (reserved, not enforced): {}",
            budget.0
        ));
    }
    if let Some(note) = &delegation.note {
        lines.push(note.as_str().to_owned());
    }
    let whole = delegation.extra.get(super::declare::NOTE_REF);
    if let Some(hex) =
        whole.and_then(|note| note.get("digest")?.as_str()?.strip_prefix(DIGEST_PREFIX))
    {
        lines.push(format!(
            "The whole note: read plan://{}/artifacts/{hex}",
            at.plan
        ));
    }
    if let Some(Value::Array(told)) = delegation.extra.get(super::brief::KEY) {
        lines.extend(told.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    lines.push(
        "When the work is done, end your turn with your answer; the engine takes it as your work."
            .to_owned(),
    );
    lines.join("\n")
}

pub(super) fn kwargs_of(
    agent: &AgentId,
    delegation: &Delegation,
    reader: bool,
) -> Result<Map<String, Value>, String> {
    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String(agent.as_str().to_owned()));
    kwargs.insert(
        "role".to_owned(),
        Value::from(if reader { "reader" } else { "root" }),
    );
    if reader {
        let partition: Vec<String> = delegation.context.iter().map(ToString::to_string).collect();
        kwargs.insert("partition".to_owned(), serde_json::json!(partition));
        if !delegation.spec.tools.is_empty() {
            kwargs.insert("tools".to_owned(), serde_json::json!(delegation.spec.tools));
        }
        if let Some(schema) = delegation.extra.get(SCHEMA) {
            kwargs.insert("schema".to_owned(), schema.clone());
        }
    }
    if let Some(model) = &delegation.spec.model {
        kwargs.insert("model".to_owned(), Value::String(model.clone()));
    }
    if let Some(effort) = delegation.spec.effort {
        kwargs.insert("thinking".to_owned(), Value::String(effort.to_string()));
    }
    match &delegation.spec.isolation {
        None | Some(Isolation::None) => {}
        Some(Isolation::Worktree) => {
            kwargs.insert("isolation".to_owned(), Value::String("worktree".to_owned()));
        }
        Some(isolation @ Isolation::Other(tag)) if isolation.container_image().is_some() => {
            kwargs.insert("isolation".to_owned(), Value::String(tag.clone()));
        }
        Some(Isolation::Other(tag)) => {
            return Err(format!(
                "delegation isolation {tag:?} has no spawn mapping; use \"none\", \"worktree\" or \"container:<image>\""
            ));
        }
    }
    if let Check::Command(command) = &delegation.accept {
        kwargs.insert("check".to_owned(), Value::String(command.clone()));
    }
    if let Some(policy) = &delegation.spec.parent_close {
        kwargs.insert("parent_close".to_owned(), serde_json::json!(policy));
    }
    // Plan section 7.4: a lease is drawn at every spawn road, so the spec's budget is the
    // engine's ask against the parent's own, refused with both numbers rather than clamped.
    if let Some(budget) = &delegation.spec.budget {
        kwargs.insert("tokens".to_owned(), Value::from(budget.0));
    }
    // Plan section 7.6: the spec's wall rides the spawn kwargs the host already reads, so a
    // plan-dispatched reader is walled at the same cooperative seams as an `rlm.run` child.
    if let Some(wall) = &delegation.spec.wall {
        for (key, list) in [
            ("deny_write", &wall.deny_write),
            ("deny_read", &wall.deny_read),
            ("deny_url", &wall.deny_url),
        ] {
            if !list.is_empty() {
                kwargs.insert(key.to_owned(), serde_json::json!(list));
            }
        }
    }
    Ok(kwargs)
}

#[cfg(test)]
mod tests {
    use super::super::dispatch::tests::delegated;
    use super::*;
    use yi_types::plan::doc::{OutputSchema, PlanId, TodoLabel};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The engine submits a child's finish (D225): a brief that named `submit` would have the
    /// child race the engine for the one record.
    #[test]
    fn a_brief_never_names_submit() -> TestResult {
        let at = TodoAddr {
            plan: PlanId::new("ship-the-thing")?,
            todo: TodoLabel::new("gateway")?,
        };
        let mut delegation = delegated("gateway")?.delegation.ok_or("delegated")?;
        for isolation in [None, Some(Isolation::None), Some(Isolation::Worktree)] {
            delegation.spec.isolation = isolation;
            let brief = brief(&at, &delegation, false);
            assert!(brief.contains("Execute todo \"gateway\""), "{brief}");
            assert!(!brief.contains("submit"), "{brief}");
            assert!(
                brief.ends_with("the engine takes it as your work."),
                "{brief}"
            );
        }
        Ok(())
    }

    /// Dies with the brief dropping the stored note: the child reads the head alone.
    #[test]
    fn an_over_cap_note_is_linked_from_the_brief() -> TestResult {
        let at = TodoAddr {
            plan: PlanId::new("ship-the-thing")?,
            todo: TodoLabel::new("quota")?,
        };
        let mut delegation = delegated("quota")?.delegation.ok_or("delegated")?;
        let hex = "ab".repeat(32);
        delegation.extra.insert(
            super::super::declare::NOTE_REF.to_owned(),
            serde_json::json!({"digest": format!("sha256:{hex}"), "media_type": "text/markdown", "length": 2000}),
        );
        let brief = brief(&at, &delegation, false);
        assert!(
            brief.contains(&format!("read plan://ship-the-thing/artifacts/{hex}")),
            "{brief}"
        );
        Ok(())
    }

    /// Dies with the wall block in `kwargs_of`: drop it and a plan-dispatched reader spawns
    /// with the parent's whole capability set (plan section 7.6).
    #[test]
    fn kwargs_carry_the_wall() -> TestResult {
        let mut delegation = delegated("read the docs")?.delegation.ok_or("delegated")?;
        delegation.spec.wall = Some(yi_types::plan::doc::WallSpec {
            deny_write: vec![".".to_owned()],
            deny_read: vec!["secrets/".to_owned()],
            deny_url: vec!["history://main".to_owned()],
        });
        let kwargs = kwargs_of(&AgentId::new("reader-1")?, &delegation, false)?;
        assert_eq!(kwargs["deny_write"], serde_json::json!(["."]));
        assert_eq!(kwargs["deny_read"], serde_json::json!(["secrets/"]));
        assert_eq!(kwargs["deny_url"], serde_json::json!(["history://main"]));
        let wall = crate::wall::Wall::from_kwargs(&kwargs, std::path::Path::new("/tmp"))?;
        assert!(
            !wall.is_empty(),
            "the host reads the same keys it is handed"
        );
        delegation.spec.wall = None;
        let bare = kwargs_of(&AgentId::new("reader-2")?, &delegation, false)?;
        assert!(!bare.contains_key("deny_write") && !bare.contains_key("deny_url"));
        Ok(())
    }

    /// Dies with the budget block in `kwargs_of`: leave it out of the kwargs and the engine's
    /// own spawn road mints tokens the parent never held, whatever `rlm.run` is refused.
    #[test]
    fn kwargs_draw_the_specs_budget_as_the_childs_lease() -> TestResult {
        let mut delegation = delegated("read the docs")?.delegation.ok_or("delegated")?;
        delegation.spec.budget = Some(yi_types::plan::doc::TokenBudget(4_000));
        let kwargs = kwargs_of(&AgentId::new("reader-1")?, &delegation, false)?;
        assert_eq!(kwargs["tokens"], serde_json::json!(4_000));
        let ask = crate::lease::Ask::from_kwargs(&kwargs)?;
        assert_eq!(
            ask.tokens,
            Some(4_000),
            "the host reads the key it is handed"
        );
        delegation.spec.budget = None;
        assert!(!kwargs_of(&AgentId::new("reader-2")?, &delegation, false)?.contains_key("tokens"));
        Ok(())
    }

    struct Served(&'static str);

    impl OutputResolve for Served {
        fn resolve(&self, _url: &yi_types::url::Url) -> Result<Option<String>, String> {
            Ok(Some(self.0.to_owned()))
        }
    }

    /// Dies with the declared output's branch dropped: a tool-declared Reader is sent no schema
    /// though `done` validates against one; and a schema that is no object would refuse the spawn.
    #[test]
    fn a_declared_output_schema_is_the_routed_readers_schema() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-spawn-schema")?;
        let store = super::super::store::PlanStore::open(dir.to_path_buf())?;
        let artifacts = store.artifacts(&PlanId::new("read-the-notes")?);
        let mut delegation = delegated("read")?.delegation.ok_or("delegated")?;
        delegation.output = Some(OutputSchema {
            schema: "local://answer.schema.json".parse()?,
            extra: Map::new(),
        });
        let mut todo = Todo::pending(TodoLabel::new("read")?);
        todo.delegation = Some(delegation);
        let served = Served(r#"{"type": "object", "required": ["answer"]}"#);
        assert_eq!(
            schema(&artifacts, &todo, Some(&served)),
            Some(serde_json::json!({"type": "object", "required": ["answer"]}))
        );
        assert_eq!(schema(&artifacts, &todo, Some(&Served("30"))), None);
        Ok(())
    }
}
