use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;

use crate::goal::DeliverFn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleScope {
    Text,
    AnyTool,
    Tool(String),
}

/// `Once` = one fire per session: a gate denies the first attempt and lets
/// the informed retry through; a reminder speaks once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleGap {
    Once,
    AfterTurns(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleMode {
    Remind,
    Gate,
}

/// A user-authored triggered rule: the runtime is a matcher delivering the
/// user's own words at the moment they apply, never a reviewer (D50 thread).
#[derive(Debug, Clone)]
pub struct RuleDoc {
    pub name: String,
    pub body: String,
    pub path: PathBuf,
    pub needles: Vec<String>,
    pub scope: RuleScope,
    pub gap: RuleGap,
    pub mode: RuleMode,
}

pub struct RuleSet {
    pub rules: Vec<RuleDoc>,
    /// A malformed rule is skipped with its reason named, never silently.
    pub warnings: Vec<String>,
}

/// Project root, then global; a project rule shadows a global one by name.
/// Yi ships zero builtin rules (zero-start law).
pub fn roots(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![home.join(".yi/rules"), cwd.join(".yi/rules")]
}

pub fn discover(cwd: &Path, home: &Path) -> RuleSet {
    let mut found: BTreeMap<String, RuleDoc> = BTreeMap::new();
    let mut warnings = Vec::new();
    for root in roots(cwd, home) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .collect();
        paths.sort();
        for path in paths {
            match read_rule(&path) {
                Ok(rule) => {
                    found.insert(rule.name.clone(), rule);
                }
                Err(reason) => warnings.push(format!("rule {} skipped: {reason}", path.display())),
            }
        }
    }
    RuleSet {
        rules: found.into_values().collect(),
        warnings,
    }
}

fn read_rule(path: &Path) -> Result<RuleDoc, String> {
    let source = std::fs::read_to_string(path).map_err(|error| format!("unreadable: {error}"))?;
    let fields = crate::skills::frontmatter(&source);
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or("no file name")?;
    let needles: Vec<String> = fields
        .get("trigger")
        .map(|trigger| {
            trigger
                .split(',')
                .map(str::trim)
                .filter(|needle| !needle.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if needles.is_empty() {
        return Err("no `trigger:` frontmatter (literal substring, comma-separated)".to_owned());
    }
    let scope = match fields.get("scope").map(String::as_str) {
        None => RuleScope::AnyTool,
        Some("text") => RuleScope::Text,
        Some("tool") => RuleScope::AnyTool,
        Some(other) => match other.strip_prefix("tool:") {
            Some(tool) if !tool.trim().is_empty() => RuleScope::Tool(tool.trim().to_owned()),
            _ => {
                return Err(format!(
                    "unknown scope `{other}` (text | tool | tool:<name>)"
                ));
            }
        },
    };
    let gap = match fields.get("gap").map(String::as_str) {
        None | Some("once") => RuleGap::Once,
        Some(turns) => match turns.parse::<u64>() {
            Ok(turns) if turns > 0 => RuleGap::AfterTurns(turns),
            _ => {
                return Err(format!(
                    "gap `{turns}` is not `once` or a positive turn count"
                ));
            }
        },
    };
    let mode = match fields.get("mode").map(String::as_str) {
        None | Some("remind") => RuleMode::Remind,
        Some("gate") => RuleMode::Gate,
        Some(other) => return Err(format!("unknown mode `{other}` (remind | gate)")),
    };
    if mode == RuleMode::Gate && scope == RuleScope::Text {
        return Err("a gate rule needs a tool scope; text cannot be denied".to_owned());
    }
    let body = source
        .split_once("---")
        .and_then(|(_, rest)| rest.split_once("---"))
        .map_or(source.as_str(), |(_, body)| body)
        .trim()
        .to_owned();
    if body.is_empty() {
        return Err("empty rule body".to_owned());
    }
    Ok(RuleDoc {
        name,
        body,
        path: path.to_path_buf(),
        needles,
        scope,
        gap,
        mode,
    })
}

#[derive(Default)]
struct FireState {
    turn: u64,
    last_fired: BTreeMap<String, u64>,
    fired_once: BTreeMap<String, bool>,
}

impl FireState {
    fn eligible(&self, rule: &RuleDoc) -> bool {
        match rule.gap {
            RuleGap::Once => !self.fired_once.get(&rule.name).copied().unwrap_or(false),
            RuleGap::AfterTurns(gap) => self
                .last_fired
                .get(&rule.name)
                .is_none_or(|fired| self.turn.saturating_sub(*fired) >= gap),
        }
    }

    fn mark(&mut self, rule: &RuleDoc) {
        self.fired_once.insert(rule.name.clone(), true);
        self.last_fired.insert(rule.name.clone(), self.turn);
    }
}

/// The match layer: literal substrings over tool arguments at the gate and
/// assistant prose at the boundary. Regex waits on the C2 decision.
pub struct RuleEngine {
    rules: Vec<RuleDoc>,
    state: Mutex<FireState>,
    deliver: Mutex<Option<DeliverFn>>,
}

fn matches(rule: &RuleDoc, haystack: &str) -> bool {
    rule.needles.iter().any(|needle| haystack.contains(needle))
}

fn tool_in_scope(rule: &RuleDoc, tool: &str) -> bool {
    match &rule.scope {
        RuleScope::Text => false,
        RuleScope::AnyTool => true,
        RuleScope::Tool(name) => name == tool,
    }
}

impl RuleEngine {
    pub fn new(rules: Vec<RuleDoc>) -> Self {
        Self {
            rules,
            state: Mutex::new(FireState::default()),
            deliver: Mutex::new(None),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn set_deliver(&self, deliver: DeliverFn) {
        if let Ok(mut slot) = self.deliver.lock() {
            *slot = Some(deliver);
        }
    }

    /// Pre-execution: a matching eligible gate rule denies the call with the
    /// rule body as evidence; matching remind rules queue for the boundary.
    pub fn check_tool(&self, tool: &str, args_json: &str) -> Option<String> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let mut denial = None;
        let mut reminders = Vec::new();
        for rule in &self.rules {
            if !tool_in_scope(rule, tool) || !matches(rule, args_json) || !state.eligible(rule) {
                continue;
            }
            match rule.mode {
                RuleMode::Gate if denial.is_none() => {
                    state.mark(rule);
                    denial = Some(format!(
                        "Denied by rule `{}` ({}):\n{}",
                        rule.name,
                        rule.path.display(),
                        rule.body
                    ));
                }
                RuleMode::Gate => {}
                RuleMode::Remind => {
                    state.mark(rule);
                    reminders.push(self.render_reminder(rule));
                }
            }
        }
        drop(state);
        self.deliver_reminders(reminders);
        denial
    }

    fn render_reminder(&self, rule: &RuleDoc) -> String {
        format!("<rule name=\"{}\">\n{}\n</rule>", rule.name, rule.body)
    }

    fn deliver_reminders(&self, reminders: Vec<String>) {
        if reminders.is_empty() {
            return;
        }
        let Some(deliver) = self
            .deliver
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
        else {
            return;
        };
        // The per-rule gap latch is the noise budget. The advisor guard is
        // deliberately NOT in this path: its session-scoped dedupe would
        // silently override a user-chosen re-arm gap (found by test).
        for text in reminders {
            deliver(
                AgentMessage::Custom {
                    custom_type: "reminder".to_owned(),
                    content: UserContent::Text(text),
                    display: true,
                    details: None,
                    timestamp: yi_session::now_ms(),
                },
                DeliveryMode::Steer,
            );
        }
    }

    /// Boundary pass: assistant prose triggers, then the turn advances.
    pub fn observe(&self, event: &AgentEvent) {
        let AgentEvent::MessageEnd {
            message:
                AgentMessage::Assistant {
                    content,
                    stop_reason: StopReason::Stop | StopReason::ToolUse,
                    ..
                },
        } = event
        else {
            return;
        };
        let text: String = content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut reminders = Vec::new();
        if let Ok(mut state) = self.state.lock() {
            if !text.is_empty() {
                for rule in &self.rules {
                    if rule.mode == RuleMode::Remind
                        && rule.scope == RuleScope::Text
                        && matches(rule, &text)
                        && state.eligible(rule)
                    {
                        state.mark(rule);
                        reminders.push(self.render_reminder(rule));
                    }
                }
            }
            state.turn = state.turn.saturating_add(1);
        }
        self.deliver_reminders(reminders);
    }
}

/// Discovery + observer wiring; the tool gate half rides `ToolAdapter`.
pub fn attach_rules(session: &crate::AgentSession, engine: Arc<RuleEngine>) {
    let steer = session.heartbeat_hook();
    engine.set_deliver(Arc::new(move |message, _mode| {
        steer(message, DeliveryMode::Steer);
    }));
    let mut events = session.subscribe();
    let observer = Arc::clone(&engine);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => observer.observe(&event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
