use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_types::message::{AgentMessage, UserContent};
use yi_types::subagent::{ChildResult, Discovery};

use crate::subagent::{ChildStatus, PARENT_NAME, SubagentHost, last_assistant_text};

pub(crate) const WAIT_MIN_MS: u64 = 1_000;
pub(crate) const WAIT_MAX_MS: u64 = 300_000;
const WAIT_POLL_MS: u64 = 100;

const CONTEXT_MAX_KEYS: usize = 8;
const CONTEXT_VALUE_CAP: usize = 4_096;
const CONTEXT_TOTAL_CAP: usize = 16_384;
const RESULT_TAIL_CHARS: usize = 2_000;
/// Invariant: every row can run an ancestor check inside the parent's own `rlm.result` call,
/// so the list is capped: a degenerate child buys one refusal, not unbounded checks.
const MAX_DISCOVERIES: usize = 16;

fn clamp(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_owned();
    }
    let kept: String = text.chars().take(cap).collect();
    format!("{kept}… [truncated to {cap} chars]")
}

/// Incident: values arrive pre-serialized because a host-side kernel read would queue behind
/// the cell awaiting this reply; the caps re-apply because that python is semi-trusted.
pub(crate) fn context_block(kwargs: &Map<String, Value>) -> Result<Option<String>, String> {
    let Some(value) = kwargs.get("context").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let entries = value
        .as_object()
        .ok_or("rlm.run context must be an object of variable name to serialized value")?;
    if entries.len() > CONTEXT_MAX_KEYS {
        return Err(format!(
            "rlm.run context names {} keys; at most {CONTEXT_MAX_KEYS} are scoped into a child",
            entries.len()
        ));
    }
    let mut block = String::from("<parent_context>\n");
    for (name, value) in entries {
        let text = value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        block.push_str(&format!("{name} = {}\n", clamp(&text, CONTEXT_VALUE_CAP)));
    }
    block = clamp(&block, CONTEXT_TOTAL_CAP);
    block.push_str("</parent_context>");
    Ok(Some(block))
}

/// Invariant: an agent's words reach another agent inside this envelope and
/// never as bare user text — provenance the model can see (B6 hardening).
fn envelope(from: &str, text: &str) -> String {
    format!("<agent_message from=\"{from}\">\n{text}\n</agent_message>")
}

fn agent_message(from: &str, text: &str) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: "agent_message".to_owned(),
        content: yi_types::message::UserContent::Text(envelope(from, text)),
        display: true,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn receipt(target: &str, state: &str) -> Value {
    json!({"target": target, "state": state})
}

pub(crate) fn message_params(payload: &Map<String, Value>) -> (String, Option<String>, bool) {
    let target = payload
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let text = payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let followup = payload
        .get("followup")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (target, text, followup)
}

/// The child half of B6: a name is looked for locally, then in the family.
pub fn register_child_messaging(
    link: ParentLink,
    local: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
) {
    let sender = link.clone();
    let own = Arc::clone(local);
    registry.register("agent_message.send", move |payload| {
        let (target, text, followup) = message_params(&payload);
        let sender = sender.clone();
        let own = Arc::clone(&own);
        Box::pin(async move {
            let text = text.ok_or("agent_message.send requires a message")?;
            if matches!(target.as_str(), "parent" | "all") {
                return sender.send(&target, &text, followup);
            }
            own.route(PARENT_NAME, &target, &text, followup)
                .or_else(|_| sender.send(&target, &text, followup))
        })
    });
    registry.register("agent_message.list_agents", move |_payload| {
        let reply = link.roster();
        Box::pin(async move { Ok(reply) })
    });
}

/// What a child needs to reach the family it was spawned into: its own name,
/// and the parent host's router (a `Weak`, so the family is not a cycle).
#[derive(Clone)]
pub struct ParentLink {
    pub child_name: String,
    pub host: std::sync::Weak<SubagentHost>,
}

impl ParentLink {
    pub fn send(
        &self,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<Map<String, Value>, String> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| "the parent session is gone".to_owned())?;
        host.route(&self.child_name, target, text, followup)
    }

    pub fn roster(&self) -> Map<String, Value> {
        self.host
            .upgrade()
            .map(|host| host.roster())
            .unwrap_or_default()
    }
}

impl SubagentHost {
    /// B6 routing, one seam for all three directions: a child reaches its
    /// parent or a named sibling, the parent reaches one child or `all`.
    pub fn route(
        &self,
        from: &str,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<Map<String, Value>, String> {
        if target.trim().is_empty() {
            return Err(
                "agent_message.send needs a target (a name, \"parent\", or \"all\")".to_owned(),
            );
        }
        if target == from {
            return Err(format!("agent \"{from}\" cannot send to itself"));
        }
        match target {
            "parent" => {
                if from == PARENT_NAME {
                    return Err("the parent has no parent to message".to_owned());
                }
                self.deliver_to_parent(from, text);
                let mut reply = Map::new();
                reply.insert(
                    "receipts".to_owned(),
                    json!([receipt("parent", "delivered")]),
                );
                Ok(reply)
            }
            "all" => {
                let names: Vec<String> = self
                    .children
                    .lock()
                    .map(|children| {
                        children
                            .values()
                            .map(|record| record.session_name.clone())
                            .filter(|name| name != from)
                            .collect()
                    })
                    .unwrap_or_default();
                // allSettled: one unreachable target never voids the fan-out.
                let receipts: Vec<Value> = names
                    .iter()
                    .map(
                        |name| match self.deliver_to_child(from, name, text, followup) {
                            Ok(state) => receipt(name, state),
                            Err(error) => receipt(name, &error),
                        },
                    )
                    .collect();
                let mut reply = Map::new();
                reply.insert("receipts".to_owned(), Value::Array(receipts));
                Ok(reply)
            }
            name => {
                let state = self.deliver_to_child(from, name, text, followup)?;
                let mut reply = Map::new();
                reply.insert("receipts".to_owned(), json!([receipt(name, state)]));
                Ok(reply)
            }
        }
    }

    fn deliver_to_parent(&self, from: &str, text: &str) {
        if let Ok(mut children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, from)
            && let Some(record) = children.get_mut(&key)
        {
            record.pending = record.pending.saturating_add(1);
            record.replied = true;
        }
        (self.options.report)(agent_message(from, text));
    }

    fn deliver_to_child(
        &self,
        from: &str,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<&'static str, String> {
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned".to_owned())?;
        let key = Self::key_of(&children, target).map_err(|_| {
            let known: Vec<&str> = children
                .values()
                .map(|record| record.session_name.as_str())
                .collect();
            format!(
                "no agent named \"{target}\"; known children: {}",
                if known.is_empty() {
                    "(none)".to_owned()
                } else {
                    known.join(", ")
                }
            )
        })?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("no agent named \"{target}\""))?;
        let message = agent_message(from, text);
        if followup {
            record.session.deliver(message);
            Ok("delivered")
        } else {
            record.session.follow_up_message(message);
            Ok("queued")
        }
    }

    pub fn roster(&self) -> Map<String, Value> {
        let mut agents = vec![json!({"name": PARENT_NAME, "role": "parent"})];
        if let Ok(children) = self.children.lock() {
            let mut names: Vec<(&str, &'static str)> = children
                .values()
                .map(|record| (record.session_name.as_str(), record.status.as_str()))
                .collect();
            names.sort_unstable();
            agents.extend(
                names
                    .into_iter()
                    .map(|(name, status)| json!({"name": name, "role": "child", "status": status})),
            );
        }
        let mut reply = Map::new();
        reply.insert("agents".to_owned(), Value::Array(agents));
        reply
    }

    /// B13 wait: which children moved since the last call; the clamp is reported.
    pub async fn wait(&self, timeout_ms: u64) -> Map<String, Value> {
        let clamped = timeout_ms.clamp(WAIT_MIN_MS, WAIT_MAX_MS);
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(clamped))
            .unwrap_or_else(std::time::Instant::now);
        loop {
            let updated = self.take_pending();
            if !updated.is_empty() || std::time::Instant::now() >= deadline {
                let mut reply = Map::new();
                reply.insert("updated".to_owned(), json!(updated));
                reply.insert("timeout_ms".to_owned(), Value::from(clamped));
                reply.insert("clamped".to_owned(), Value::Bool(clamped != timeout_ms));
                return reply;
            }
            tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS)).await;
        }
    }

    fn take_pending(&self) -> Vec<String> {
        let Ok(mut children) = self.children.lock() else {
            return Vec::new();
        };
        let mut moved: Vec<String> = children
            .values_mut()
            .filter(|record| record.pending > 0)
            .map(|record| {
                record.pending = 0;
                record.session_name.clone()
            })
            .collect();
        moved.sort();
        moved
    }

    /// B13 interrupt: ends the run and keeps the record, unlike delete.
    pub fn interrupt(&self, target: &str) -> Result<Map<String, Value>, String> {
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        record.session.abort();
        let mut reply = Map::new();
        reply.insert("interrupted".to_owned(), Value::String(key));
        Ok(reply)
    }

    /// The child's answer as data in the parent's namespace: JSON when it parses, checked
    /// against `schema` if given, a whole [`yi_types::subagent::ChildResult`] if check-spawned.
    pub fn result(
        &self,
        target: &str,
        schema: Option<&Value>,
    ) -> Result<Map<String, Value>, String> {
        let (name, check, text) = {
            let children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let key = Self::key_of(&children, target)?;
            let record = children
                .get(&key)
                .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
            if record.status == ChildStatus::Running {
                return Err(format!("child \"{target}\" is still running"));
            }
            if let Some(error) = &record.error {
                return Err(format!("child \"{target}\" failed: {error}"));
            }
            (
                record.session_name.clone(),
                record.check.clone(),
                last_assistant_text(&record.session.messages()).unwrap_or_default(),
            )
        };
        let json = serde_json::from_str::<Value>(text.trim()).ok();
        if let Some(schema) = schema {
            let schema = crate::schema::Schema::from_value(schema.clone())
                .map_err(|error| format!("child \"{target}\" schema rejected: {error}"))?;
            let value = json.clone().ok_or_else(|| {
                format!(
                    "child \"{target}\" answered with text, not JSON, and a schema was given so JSON is required:\n{text}"
                )
            })?;
            schema
                .validate(&value)
                .map_err(|error| format!("child \"{target}\" result rejected: {error}"))?;
        }
        let mut reply = Map::new();
        reply.insert("name".to_owned(), Value::String(name.clone()));
        reply.insert("text".to_owned(), Value::String(text.clone()));
        if let Some(json) = json {
            reply.insert("json".to_owned(), json);
        }
        if let Some(check) = check {
            let result = serde_json::from_str::<ChildResult>(text.trim()).map_err(|error| {
                format!(
                    "child \"{target}\" was spawned with a check, so it owes a result object {{\"value\": …, \"discoveries\": [ … ]}}: {error}\nIts answer was:\n{}",
                    clamp(&text, RESULT_TAIL_CHARS)
                )
            })?;
            if result.discoveries.len() > MAX_DISCOVERIES {
                return Err(format!(
                    "child \"{target}\" reported {} discoveries; at most {MAX_DISCOVERIES} are adjudicated in one result — keep the rest in the value",
                    result.discoveries.len()
                ));
            }
            crate::goal::run_check(&check, crate::goal::DEFAULT_CHECK_TIMEOUT_MS).map_err(
                |evidence| {
                    format!("child \"{target}\" result held back; its check is red: {evidence}")
                },
            )?;
            self.route_discoveries(&name, &result.discoveries)
                .map_err(|error| format!("child \"{target}\" result held back; {error}"))?;
            reply.insert(
                "discoveries".to_owned(),
                serde_json::to_value(&result.discoveries)
                    .unwrap_or_else(|_| Value::Array(Vec::new())),
            );
            reply.insert("value".to_owned(), result.value);
        }
        Ok(reply)
    }

    /// L5 criticality is derived, never declared: the ancestor todo is looked up, its check
    /// re-run, and only a red one is HIGH. Fails closed: an unadjudicable row holds the result.
    fn route_discoveries(&self, child: &str, discoveries: &[Discovery]) -> Result<(), String> {
        let plan = if discoveries
            .iter()
            .any(|row| row.violates_check_of.is_some())
        {
            Some(self.ancestor_plan()?)
        } else {
            None
        };
        // One check run per named todo, however many rows name it.
        let mut adjudged: HashMap<&yi_types::plan::TaskId, Option<String>> = HashMap::new();
        for row in discoveries {
            let named = row.violates_check_of.as_ref().and_then(|id| {
                plan.as_ref()
                    .and_then(|plan| crate::plan::todo_check(plan, id.as_str()))
                    .map(|check| (id, check))
            });
            let red = named.and_then(|(id, check)| {
                adjudged
                    .entry(id)
                    .or_insert_with(|| {
                        crate::goal::run_check(&check, crate::goal::DISCOVERY_CHECK_TIMEOUT_MS)
                            .err()
                    })
                    .clone()
                    .map(|evidence| (id, evidence))
            });
            let mut details = json!({
                "criticality": if red.is_some() { "high" } else { "deferred" },
                "child": child,
                "discovery": row,
            });
            let text = match red {
                Some((id, evidence)) => {
                    crate::goal::record_discovery(&self.options.store, row).map_err(|error| {
                        format!(
                            "the HIGH discovery {} could not be recorded, so the completion gate would never see it: {error}",
                            row.fingerprint
                        )
                    })?;
                    if let Some(map) = details.as_object_mut() {
                        map.insert("task".to_owned(), Value::String(id.as_str().to_owned()));
                        map.insert("evidence".to_owned(), Value::String(evidence.clone()));
                    }
                    format!(
                        "HIGH discovery from {child}: {}\nThe check of task {} is red — address it before continuing:\n{evidence}",
                        row.text,
                        id.as_str()
                    )
                }
                None => format!("deferred discovery from {child}: {}", row.text),
            };
            (self.options.report)(AgentMessage::Custom {
                custom_type: "discovery".to_owned(),
                content: UserContent::Text(text),
                display: true,
                details: Some(details),
                timestamp: yi_session::now_ms(),
            });
        }
        Ok(())
    }

    /// Invariant: an unreadable canonical plan is an adjudication failure, never a deferred
    /// row — a check-violating discovery must not be downgraded by a missing reader.
    fn ancestor_plan(&self) -> Result<yi_types::plan::doc::Plan, String> {
        crate::plan::canonical_plan(&self.options.store, &self.options.plans_dir)
            .map_err(|cause| {
                format!(
                    "a discovery names an ancestor check but the canonical plan cannot be read ({cause}), so criticality cannot be derived"
                )
            })
    }

    /// Invariant: promotion runs at reap whatever the outcome, so the last product reaches
    /// the owner's transcript before the slot frees and no live child dangles.
    pub fn reap(&self, target: &str) -> Result<Harvest, String> {
        let record = {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let key = Self::key_of(&children, target)?;
            children
                .remove(&key)
                .ok_or_else(|| format!("No RLM child matches \"{target}\""))?
        };
        if record.status == ChildStatus::Running {
            record.session.abort();
        }
        SubagentHost::dispose_child_kernel(&record.session);
        let answer = last_assistant_text(&record.session.messages());
        let body = match (&record.error, &answer) {
            (Some(error), Some(answer)) => {
                format!(
                    "failed: {error}\nlast product:\n{}",
                    clamp(answer, RESULT_TAIL_CHARS)
                )
            }
            (Some(error), None) => format!("failed: {error}"),
            (None, Some(answer)) => clamp(answer, RESULT_TAIL_CHARS),
            (None, None) => "(no product)".to_owned(),
        };
        let name = record.session_name.clone();
        if let Some(store) = record.session.store()
            && let Ok(mut reaped) = self.reaped.lock()
        {
            reaped.insert(name.clone(), store);
        }
        (self.options.report)(AgentMessage::Custom {
            custom_type: "reap".to_owned(),
            content: UserContent::Text(format!(
                "<reaped_child from=\"{name}\">\n{body}\n</reaped_child>"
            )),
            display: true,
            details: Some(json!({
                "child": name,
                "status": record.status.as_str(),
                "error": record.error,
            })),
            timestamp: yi_session::now_ms(),
        });
        Ok(Harvest {
            name,
            produced: answer.is_some(),
        })
    }
}

/// What a reap found: the child's name and whether it left any product to
/// point a terminal record at.
pub struct Harvest {
    pub name: String,
    pub produced: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use yi_types::plan::TaskId;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn scratch(tag: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!(
            "yi-mailbox-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn memory_store_with_goal() -> Result<yi_session::SharedSession, Box<dyn std::error::Error>> {
        let store: yi_session::SharedSession = Arc::new(Mutex::new(
            yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
                id: "mailbox-test".to_owned(),
                created_at: 0,
                parent_session_id: None,
                name: None,
            }),
        ));
        yi_session::lock_session(&store).set_goal(yi_types::goal::Goal {
            objective: "adjudicate".to_owned(),
            status: yi_types::goal::GoalStatus::Active,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            created: 0,
            updated: 0,
            check: None,
            check_timeout_ms: None,
            check_failure: None,
            discoveries: Vec::new(),
            extra: Map::new(),
        })?;
        Ok(store)
    }

    fn write_canonical_plan(cwd: &std::path::Path, check: &str) -> TestResult {
        use yi_types::plan::doc::{
            Check, Delegation, GoalText, Plan, PlanId, PlanTier, RetryCount, SpawnSpec, Todo,
            TodoLabel, TodoState,
        };
        let store = crate::plan::store::PlanStore::open(cwd.join(crate::plan::PLANS_DIR))?;
        let plan = Plan::opening(
            PlanId::new("adjudication")?,
            GoalText::new("adjudicate discoveries")?,
            PlanTier::Root,
            vec![Todo {
                label: TodoLabel::new("t1")?,
                after: Vec::new(),
                state: TodoState::Pending,
                delegation: Some(Delegation {
                    spec: SpawnSpec {
                        role: None,
                        model: None,
                        effort: None,
                        tools: Vec::new(),
                        isolation: None,
                        budget: None,
                        extra: Map::new(),
                    },
                    accept: Check::Command(check.to_owned()),
                    output: None,
                    context: Vec::new(),
                    note: None,
                    extra: Map::new(),
                }),
                subplan: None,
                retries: RetryCount::default(),
                children: Vec::new(),
                extra: Map::new(),
            }],
        );
        store.write(&crate::plan::store::PlanFile {
            plan,
            body: String::new(),
        })?;
        Ok(())
    }

    type Sink = Arc<Mutex<Vec<AgentMessage>>>;

    fn host_at(
        cwd: PathBuf,
        store: yi_session::SharedSession,
    ) -> Result<(Arc<SubagentHost>, Sink), Box<dyn std::error::Error>> {
        let (events, _keep) = tokio::sync::broadcast::channel(16);
        let reports: Sink = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reports);
        let plans_dir = cwd.join(crate::plan::PLANS_DIR);
        let host = Arc::new(SubagentHost::new(crate::subagent::SubagentHostOptions {
            depth: 0,
            max_depth: 1,
            max_children: 8,
            parent_session_dir: cwd.join("children"),
            cwd,
            home: std::env::temp_dir(),
            lane_slots: 1,
            defaults: Arc::new(|| {
                (
                    yi_types::model::Model {
                        id: "faux-1".to_owned(),
                        name: "Faux".to_owned(),
                        api: "faux".to_owned(),
                        provider: "faux".to_owned(),
                        base_url: "http://localhost:0".to_owned(),
                        reasoning: false,
                        input: vec!["text".to_owned()],
                        cost: yi_types::model::ModelCost {
                            input: serde_json::Number::from(0u64),
                            output: serde_json::Number::from(0u64),
                            cache_read: serde_json::Number::from(0u64),
                            cache_write: serde_json::Number::from(0u64),
                            tiers: None,
                        },
                        context_window: 128_000,
                        max_tokens: 16_384,
                        compat: None,
                        thinking_level_map: None,
                        headers: None,
                    },
                    yi_types::model::Effort::Medium,
                )
            }),
            factory: Arc::new(|_build| Err("no child in this test".to_owned())),
            notice: Arc::new(|_text: &str| {}),
            events,
            parent_messages: Arc::new(Vec::new),
            report: Arc::new(move |message| {
                if let Ok(mut queue) = sink.lock() {
                    queue.push(message);
                }
            }),
            attribute: Arc::new(|_usage| {}),
            store: Arc::new(move || Some(store.clone())),
            plans_dir,
        }));
        Ok((host, reports))
    }

    fn row(names_check: bool) -> Discovery {
        Discovery {
            text: "the retry loop double-counts".to_owned(),
            violates_check_of: names_check.then(|| TaskId("t1".to_owned())),
            fingerprint: "aaa".to_owned(),
            extra: Map::new(),
        }
    }

    #[test]
    fn a_discovery_is_adjudicated_against_the_canonical_document() -> TestResult {
        let cwd = scratch("adjudicate")?;
        write_canonical_plan(&cwd, "echo t1 broken; exit 4")?;
        let store = memory_store_with_goal()?;
        let (host, reports) = host_at(cwd, store.clone())?;
        host.route_discoveries("finder", &[row(true)])?;
        let texts: Vec<String> = reports
            .lock()
            .map_err(|_| "poisoned")?
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Custom {
                    content: UserContent::Text(text),
                    ..
                } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|text| {
                text.starts_with("HIGH discovery from finder") && text.contains("t1 broken")
            }),
            "a red canonical check derives HIGH with the evidence: {texts:?}"
        );
        let goal = yi_session::lock_session(&store).goal().ok_or("goal")?;
        assert_eq!(
            goal.discoveries.len(),
            1,
            "the HIGH row lands on the goal ledger so completion stays gated"
        );
        Ok(())
    }

    #[test]
    fn a_green_canonical_check_defers_the_row() -> TestResult {
        let cwd = scratch("defer")?;
        write_canonical_plan(&cwd, "true")?;
        let (host, reports) = host_at(cwd, memory_store_with_goal()?)?;
        host.route_discoveries("finder", &[row(true)])?;
        let texts: Vec<String> = reports
            .lock()
            .map_err(|_| "poisoned")?
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Custom {
                    content: UserContent::Text(text),
                    ..
                } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("deferred discovery from finder")),
            "{texts:?}"
        );
        Ok(())
    }

    #[test]
    fn adjudication_fails_closed_when_no_canonical_plan_is_readable() -> TestResult {
        let cwd = scratch("fail-closed")?;
        let (host, reports) = host_at(cwd, memory_store_with_goal()?)?;
        let error = host
            .route_discoveries("finder", &[row(true)])
            .err()
            .ok_or("a check-naming row with no readable plan must hold the result back")?;
        assert!(
            error.contains("canonical plan cannot be read"),
            "the refusal names the missing reader: {error}"
        );
        assert!(
            reports.lock().map_err(|_| "poisoned")?.is_empty(),
            "nothing is routed when adjudication is impossible"
        );
        Ok(())
    }
}
