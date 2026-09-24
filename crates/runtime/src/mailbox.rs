use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_types::event::AgentEvent;
use yi_types::mail::{Delivery, Kind, Receipt};
use yi_types::message::{AgentMessage, UserContent};
use yi_types::subagent::{ChildResult, Discovery};

use crate::family::Cause;
use crate::mail::{Desk, Draft};
use crate::subagent::{
    ChildExit, ChildRecord, INTERRUPTED, PARENT_NAME, Step, SubagentHost, last_assistant_text,
};

pub(crate) const WAIT_MIN_MS: u64 = 1_000;
pub(crate) const WAIT_MAX_MS: u64 = 300_000;
const WAIT_POLL_MS: u64 = 100;

pub(crate) const CONTEXT_MAX_KEYS: usize = 8;
pub(crate) const CONTEXT_VALUE_CAP: usize = 4_096;
pub(crate) const CONTEXT_TOTAL_CAP: usize = 16_384;
const RESULT_TAIL_CHARS: usize = 2_000;

pub(crate) type Retired = (
    String,
    ChildRecord,
    Option<(yi_types::plan::op::Choice, crate::lane::settle::Candidate)>,
);
/// What `retire_as` runs between a settled lane and the record's release.
pub(crate) type Commit<'a> =
    dyn Fn(&ChildRecord, Option<&crate::lane::settle::Candidate>) -> Result<(), String> + 'a;
/// Invariant: every row can run an ancestor check inside the parent's own `rlm.result` call,
/// so the list is capped: a degenerate child buys one refusal, not unbounded checks.
pub(crate) const MAX_DISCOVERIES: usize = 16;

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

fn receipt(target: &str, state: &str) -> Value {
    json!({"target": target, "state": state})
}

pub(crate) fn timeout_of(payload: &Map<String, Value>) -> u64 {
    payload
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(WAIT_MAX_MS)
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
        let parsed = Draft::from_payload(&payload);
        let sender = sender.clone();
        let own = Arc::clone(&own);
        Box::pin(async move {
            let (target, draft) = parsed?;
            if matches!(target.as_str(), "parent" | "all") {
                return sender.send_mail(&target, &draft);
            }
            own.route_mail(PARENT_NAME, &target, &draft)
                .or_else(|_| sender.send_mail(&target, &draft))
        })
    });
    let (sender, own) = (link.clone(), Arc::clone(local));
    registry.register("agent_message.request", move |payload| {
        own.waited_on(true);
        let parsed = Draft::from_payload(&payload);
        let timeout = timeout_of(&payload);
        let sender = sender.clone();
        let own = Arc::clone(&own);
        Box::pin(async move {
            let (target, draft) = parsed?;
            // A name this child holds is its own child; any other is the family's to find.
            if target != "parent" && own.holds(&target) {
                return own
                    .request(PARENT_NAME, &target, &draft.text, timeout)
                    .await;
            }
            let host = sender.host.upgrade().ok_or("the parent session is gone")?;
            host.request(&sender.child_name, &target, &draft.text, timeout)
                .await
        })
    });
    registry.register("agent_message.list_agents", move |_payload| {
        let reply = link.roster();
        Box::pin(async move { Ok(reply) })
    });
}

pub fn register_receive(
    session: &crate::session::AgentSession,
    host: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
) {
    let (take, host) = (session.mail_hook(), Arc::clone(host));
    registry.register("rlm.receive", move |payload| {
        let asked = timeout_of(&payload);
        let clamped = asked.clamp(WAIT_MIN_MS, WAIT_MAX_MS);
        let (take, host) = (Arc::clone(&take), Arc::clone(&host));
        Box::pin(async move {
            let started = std::time::Instant::now();
            let deadline = std::time::Duration::from_millis(clamped);
            loop {
                let envelopes = take();
                if !envelopes.is_empty() || started.elapsed() >= deadline {
                    host.waited_on(!envelopes.is_empty());
                    let mut reply = Map::new();
                    reply.insert("envelopes".to_owned(), Value::Array(envelopes));
                    reply.insert("timeout_ms".to_owned(), Value::from(clamped));
                    reply.insert("clamped".to_owned(), Value::Bool(clamped != asked));
                    return Ok(reply);
                }
                tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS)).await;
            }
        })
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
        self.send_mail(target, &Draft::plain(text, followup))
    }

    fn send_mail(&self, target: &str, draft: &Draft) -> Result<Map<String, Value>, String> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| "the parent session is gone".to_owned())?;
        host.route_mail(&self.child_name, target, draft)
    }

    pub fn ask(&self, question: &str, cancelled: &yi_tools::CancelFlag) -> Result<String, String> {
        let host = self.host.upgrade().ok_or("the parent session is gone")?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?;
        runtime.block_on(async {
            let stopped = async {
                while !cancelled() && !host.cancelled_member(&self.child_name) {
                    tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS)).await;
                }
            };
            let asked = host.request(&self.child_name, PARENT_NAME, question, WAIT_MAX_MS);
            tokio::select! {
                answer = asked => answer.map(|reply| {
                    reply.get("reply").and_then(Value::as_str).unwrap_or_default().to_owned()
                }),
                () = stopped => Err("the run was stopped before the parent answered".to_owned()),
            }
        })
    }

    pub fn roster(&self) -> Map<String, Value> {
        self.host
            .upgrade()
            .map(|host| host.roster())
            .unwrap_or_default()
    }
}

impl SubagentHost {
    pub fn route(
        &self,
        from: &str,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<Map<String, Value>, String> {
        self.route_mail(from, target, &Draft::plain(text, followup))
    }

    /// `agent_message.send` for a sender the registry fixed: the payload names the target,
    /// the message and its kind, never who sent it.
    pub fn send(
        &self,
        from: &str,
        payload: &Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        let (target, draft) = Draft::from_payload(payload)?;
        self.route_mail(from, &target, &draft)
    }

    /// B6 routing, one seam for all three directions: a child reaches its
    /// parent or a named sibling, the parent reaches one child or `all`.
    pub(crate) fn route_mail(
        &self,
        from: &str,
        target: &str,
        draft: &Draft,
    ) -> Result<Map<String, Value>, String> {
        if target.trim().is_empty() {
            return Err(
                "agent_message.send needs a target (a name, \"parent\", or \"all\")".to_owned(),
            );
        }
        if target == from {
            return Err(format!("agent \"{from}\" cannot send to itself"));
        }
        draft.admit(from)?;
        let mut desk = self.mail.lock().map_err(|_| "mail state poisoned")?;
        desk.refuse_second_answer(draft)?;
        let receipts = match target {
            "parent" if from == PARENT_NAME => {
                return Err("the parent has no parent to message".to_owned());
            }
            "parent" => vec![self.deliver_to_parent(&mut desk, from, draft)?],
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
                names
                    .iter()
                    .map(|name| {
                        self.deliver_to_child(&mut desk, from, name, draft)
                            .unwrap_or_else(|error| receipt(name, &error))
                    })
                    .collect()
            }
            name => vec![self.deliver_to_child(&mut desk, from, name, draft)?],
        };
        let mut reply = Map::new();
        reply.insert("receipts".to_owned(), Value::Array(receipts));
        Ok(reply)
    }

    /// The name a member answers to, so a waiter keyed on it matches the reply's `from`.
    pub(crate) fn member_name(&self, target: &str) -> String {
        self.children
            .lock()
            .ok()
            .and_then(|children| {
                let key = Self::key_of(&children, target).ok()?;
                Some(children.get(&key)?.session_name.clone())
            })
            .unwrap_or_else(|| target.to_owned())
    }

    fn deliver_to_parent(
        &self,
        desk: &mut Desk,
        from: &str,
        draft: &Draft,
    ) -> Result<Value, String> {
        let mut incarnation = None;
        if let Ok(mut children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, from)
            && let Some(record) = children.get_mut(&key)
        {
            incarnation = record.standing.incarnation();
            record.step(Step::Replied);
            let cause = match draft.kind {
                Kind::Progress => Cause::Progress,
                Kind::Request => Cause::Asked,
                // The child's own verdict on its work: `wait` reports it failed from here on.
                Kind::Failure if record.step(Step::Failed(draft.text.clone())) => Cause::Failed,
                _ => Cause::Mail,
            };
            // A waiter blocked on the family returns on each envelope, so its cell reads it.
            children.touch(&key, cause);
        }
        let envelope = desk.seal((from, PARENT_NAME), (incarnation, None), draft);
        if let Some(store) = (self.options.store)() {
            crate::mail::inbox(&store, &envelope)?;
        }
        let answered = desk.resolve(&envelope, draft.by_human);
        if answered && let Some(store) = (self.options.store)() {
            crate::mail::mark_read(&store, vec![Value::String(envelope.id.0.clone())]);
        }
        if envelope.kind != Kind::Progress && !answered {
            (self.options.report)(crate::mail::present(&envelope));
        }
        let state = if answered { "answered" } else { "delivered" };
        let mut row = receipt(PARENT_NAME, state);
        row["id"] = Value::String(envelope.id.0);
        Ok(row)
    }

    fn deliver_to_child(
        &self,
        desk: &mut Desk,
        from: &str,
        target: &str,
        draft: &Draft,
    ) -> Result<Value, String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned".to_owned())?;
        let key = Self::key_of(&children, target).ok();
        let record = key.as_ref().and_then(|key| children.get(key));
        let (name, store) = match record {
            Some(record) => (record.session_name.clone(), record.session.store()),
            // A reaped child's chain is kept (D210), and that is where its inbox lives.
            None => (target.to_owned(), self.kept_transcript(target)),
        };
        let Some(store) = store else {
            let known: Vec<&str> = children
                .values()
                .map(|record| record.session_name.as_str())
                .collect();
            return Err(format!(
                "no agent named \"{target}\"; known children: {}",
                if known.is_empty() {
                    "(none)".to_owned()
                } else {
                    known.join(", ")
                }
            ));
        };
        let incarnation_of = |name: &str| {
            let key = Self::key_of(&children, name).ok()?;
            children.get(&key)?.standing.incarnation()
        };
        let between = (incarnation_of(from), incarnation_of(&name));
        let envelope = desk.seal((from, &name), between, draft);
        crate::mail::inbox(&store, &envelope)?;
        let answered = desk.resolve(&envelope, draft.by_human);
        let message = crate::mail::present(&envelope);
        // `followup` only wakes an idle receiver; a late reply wakes nobody, its asker returned.
        let wakes = envelope.kind != Kind::Cancel
            && (draft.followup
                || !matches!(envelope.kind, Kind::Inform | Kind::Progress | Kind::Reply));
        let state = match record {
            _ if answered => {
                crate::mail::mark_read(&store, vec![Value::String(envelope.id.0.clone())]);
                Delivery::Answered
            }
            _ if envelope.kind == Kind::Progress => Delivery::Inboxed,
            // A revoked child is admitted no new work; its inbox still keeps the message.
            Some(record) if record.lease.revoked.is_some() && envelope.kind != Kind::Cancel => {
                Delivery::Inboxed
            }
            // A cancel starts no turn: the flag ends a live one at its next message boundary.
            Some(record) => {
                if envelope.kind == Kind::Cancel {
                    record.session.cancel();
                }
                record.session.deliver(message, wakes)
            }
            None => Delivery::Inboxed,
        };
        if let (Delivery::Woken, Some(key)) = (state, &key)
            && let Some(record) = children.get_mut(key)
            && record.step(Step::Resumed)
        {
            children.touch(key, Cause::Started);
        }
        let row = Receipt {
            target: name,
            id: envelope.id,
            state,
        };
        serde_json::to_value(row).map_err(|error| error.to_string())
    }

    pub fn roster(&self) -> Map<String, Value> {
        let mut agents = vec![json!({"name": PARENT_NAME, "role": "parent"})];
        if let Ok(children) = self.children.lock() {
            let mut names: Vec<(&str, &'static str)> = children
                .values()
                .map(|record| {
                    let status = crate::family::read_exit(record.exit).status;
                    (record.session_name.as_str(), status.as_str())
                })
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

    /// B13 wait with a per-caller cursor (§7.5): nothing shared is drained, so no waiter steals.
    pub async fn wait(&self, timeout_ms: u64, cursor: Option<u64>) -> Map<String, Value> {
        self.wait_for(timeout_ms, cursor, false).await
    }

    pub(crate) async fn wait_for(
        &self,
        timeout_ms: u64,
        cursor: Option<u64>,
        bare: bool,
    ) -> Map<String, Value> {
        self.waited_on(false);
        let clamped = timeout_ms.clamp(WAIT_MIN_MS, WAIT_MAX_MS);
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(clamped))
            .unwrap_or_else(std::time::Instant::now);
        let since = cursor.unwrap_or(0);
        loop {
            let (epoch, moved, live) = self.changed_since(since);
            let quiet = bare && moved.is_empty() && live;
            if (epoch > since && !quiet) || std::time::Instant::now() >= deadline {
                if bare {
                    self.saw(None);
                }
                let changed: Vec<&str> = moved.keys().map(String::as_str).collect();
                let causes: Map<String, Value> = moved
                    .iter()
                    .map(|(name, cause)| (name.clone(), Value::from(cause.as_str())))
                    .collect();
                let mut states = Map::new();
                let mut notes = Map::new();
                for view in self.states() {
                    states.insert(
                        view.name.clone(),
                        Value::String(view.state.as_str().to_owned()),
                    );
                    if let Some(note) = view.note {
                        notes.insert(view.name, Value::String(note));
                    }
                }
                let mut reply = Map::new();
                reply.insert("cursor".to_owned(), Value::from(epoch));
                reply.insert("changed".to_owned(), json!(changed));
                reply.insert("causes".to_owned(), Value::Object(causes));
                reply.insert("states".to_owned(), Value::Object(states));
                reply.insert("notes".to_owned(), Value::Object(notes));
                reply.insert("updated".to_owned(), json!(changed));
                reply.insert("timeout_ms".to_owned(), Value::from(clamped));
                reply.insert("clamped".to_owned(), Value::Bool(clamped != timeout_ms));
                return reply;
            }
            tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS)).await;
        }
    }

    fn changed_since(&self, since: u64) -> (u64, std::collections::BTreeMap<String, Cause>, bool) {
        let Ok(children) = self.children.lock() else {
            return (since, std::collections::BTreeMap::new(), false);
        };
        let (gone, whole) = children.removed_since(since);
        let mut moved: std::collections::BTreeMap<String, Cause> =
            gone.into_iter().map(|name| (name, Cause::Reaped)).collect();
        moved.extend(
            children
                .values()
                .filter(|record| record.changed_at_epoch > since)
                .map(|record| (record.session_name.clone(), record.cause)),
        );
        let live = whole && children.values().any(|record| record.exit.is_none());
        (children.epoch, moved, live)
    }

    // Incident: nine of twelve F0e "text, not JSON" refusals were a valid object inside a
    // fenced block, so one fence line and any trailer are framing, not the answer (#475).
    pub(crate) fn json_answer(text: &str) -> Option<Value> {
        let body = text.trim();
        let body = match body.strip_prefix("```") {
            Some(rest) => rest
                .split_once('\n')
                .map_or(rest, |(_, body)| body)
                .trim_end()
                .trim_end_matches("```")
                .trim(),
            None => body,
        };
        serde_json::Deserializer::from_str(body)
            .into_iter::<Value>()
            .next()?
            .ok()
    }

    /// B13 interrupt: ends the run and keeps the record, unlike delete.
    pub fn interrupt(&self, target: &str) -> Result<Map<String, Value>, String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        record.session.abort();
        children.touch(&key, Cause::Interrupted);
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
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let key = Self::key_of(&children, target)?;
            let record = children
                .get_mut(&key)
                .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
            if record.exit.is_none() {
                return Err(format!("child \"{target}\" is still running"));
            }
            record.seen = record.changed_at_epoch;
            if let Some(error) = &record.error {
                return Err(format!("child \"{target}\" failed: {error}"));
            }
            (
                record.session_name.clone(),
                record.check.clone(),
                last_assistant_text(&record.session.messages()).unwrap_or_default(),
            )
        };
        let json = Self::json_answer(&text);
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
            let cwd = self
                .cwd_of(&name)
                .unwrap_or_else(|| self.options.cwd.clone());
            crate::goal::run_check(&check, &cwd, crate::goal::DEFAULT_CHECK_TIMEOUT_MS).map_err(
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
    pub(crate) fn route_discoveries(
        &self,
        child: &str,
        discoveries: &[Discovery],
    ) -> Result<(), String> {
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
                        let timeout = crate::goal::DISCOVERY_CHECK_TIMEOUT_MS;
                        crate::goal::run_check(&check, &self.options.cwd, timeout).err()
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

    /// Invariant: the one way a record leaves `children`, so every removal publishes one
    /// terminal update; a run it cut short reads `interrupted`, never finished.
    pub(crate) fn retire(&self, target: &str) -> Result<Retired, String> {
        self.retire_as(target, ChildExit::Reaped, &|_, _| Ok(()))
    }

    /// `commit` runs once the lane has settled and before anything is released or published;
    /// its refusal puts the record back, so a repossession's record is never second.
    pub(crate) fn retire_as(
        &self,
        target: &str,
        exit: ChildExit,
        commit: &Commit<'_>,
    ) -> Result<Retired, String> {
        let (key, mut record) = {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let key = Self::key_of(&children, target)?;
            // A worktree goes only through a recorded disposition: merge, discard, or the
            // engine's dispose seam, never a removal that drops the only copy of its work.
            if let Some(record) = children.get(&key)
                && let Some(tree) = &record.worktree
                && record.disposition.is_none()
            {
                return Err(format!(
                    "child \"{target}\" holds the worktree {}; rlm.merge_worktree(\"{target}\") or rlm.discard_worktree(\"{target}\") first",
                    tree.path().display()
                ));
            }
            let record = children
                .take(&key)
                .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
            (key, record)
        };
        if record.exit.is_none() {
            // Cancelled first, so the settle of the aborted run starts none for mail it queued.
            record.session.cancel();
            record.session.abort();
            // A lane refusal stays the cause of a plain removal; a repossession names itself.
            let cause = match exit {
                ChildExit::Reaped => record
                    .error
                    .take()
                    .unwrap_or_else(|| INTERRUPTED.to_owned()),
                other => crate::family::read_exit(Some(other)).verb.to_owned(),
            };
            record.step(Step::Exit(exit, Some(cause)));
        }
        SubagentHost::dispose_child_kernel(&record.session);
        // The lane settles under the journaled choice; one that cannot restores the record.
        let (mut record, settled) = self.settle_or_restore(&key, record)?;
        if let Err(reason) = commit(&record, settled.as_ref().map(|(_, candidate)| candidate)) {
            self.restore(&key, record, &reason);
            return Err(reason);
        }
        drop(record.lane_permit.take());
        self.return_lease(&record);
        let update = record.update(&key);
        let _ = self.options.events.send(AgentEvent::ChildUpdate { update });
        if let Ok(mut desk) = self.mail.lock() {
            desk.drop_respondent(
                &record.session_name,
                crate::family::read_exit(record.exit).verb,
            );
        }
        Ok((key, record, settled))
    }

    /// Invariant: promotion runs at reap whatever the outcome, so the last product reaches
    /// the owner's transcript before the slot frees and no live child dangles.
    pub fn reap(&self, target: &str) -> Result<Harvest, String> {
        let (_key, record, _settled) = self.retire(target)?;
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
        let block = format!("<reaped_child from=\"{name}\">\n{body}\n</reaped_child>");
        let harvested = self.harvests.lock().is_ok_and(|mut held| {
            held.get_mut(&name)
                .map(|slot| *slot = Some(block.clone()))
                .is_some()
        });
        if harvested {
            return Ok(Harvest {
                name,
                produced: answer.is_some(),
            });
        }
        (self.options.report)(AgentMessage::Custom {
            custom_type: "reap".to_owned(),
            content: UserContent::Text(block),
            display: true,
            details: Some(json!({
                "child": name,
                "status": crate::family::read_exit(record.exit).status.as_str(),
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

/// What a reap found: the child's name and whether it left any product to point a terminal
/// record at; how its worktree went is the engine's journaled disposition, not the reap's.
pub struct Harvest {
    pub name: String,
    pub produced: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use yi_types::plan::TaskId;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
                        wall: None,
                        parent_close: None,
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
                note: None,
                attempt: yi_types::plan::doc::AttemptId::FIRST,
                refusals: 0,
                contract: None,
                contract_hash: None,
                extra: Map::new(),
            }],
        );
        store.write(&plan)?;
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
            notice: Arc::new(|_text: &str, _| {}),
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
            family_live: Arc::new(|| 0),
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
        let cwd = Scratch::new("yi-mailbox-adjudicate")?;
        write_canonical_plan(&cwd, "echo t1 broken; exit 4")?;
        let store = memory_store_with_goal()?;
        let (host, reports) = host_at(cwd.to_path_buf(), store.clone())?;
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
        let cwd = Scratch::new("yi-mailbox-defer")?;
        write_canonical_plan(&cwd, "true")?;
        let (host, reports) = host_at(cwd.to_path_buf(), memory_store_with_goal()?)?;
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
        let cwd = Scratch::new("yi-mailbox-fail-closed")?;
        let (host, reports) = host_at(cwd.to_path_buf(), memory_store_with_goal()?)?;
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
