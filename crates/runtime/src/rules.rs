use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Attribution, StopReason, UserContent};
use yi_types::schedule::DeliveryMode;

use crate::fetch::FetchLog;
use crate::goal::DeliverFn;

// Incident: always-on skill bodies were the cost regression; two pointers a message is the budget.
// It caps skill pointers only: a user's rule is their words, and a typed `$name` a request.
const POINTER_CAP: usize = 2;
/// A skill pointer is this prefix plus the skill's name, an address `read` serves.
pub(crate) const SKILL_ADDRESS: &str = "yi://skills/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleScope {
    Text,
    AnyTool,
    Tool(String),
    Result,
    Error,
}

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

/// Invariant: the body is the user's words delivered verbatim, never generated judgment.
#[derive(Debug, Clone)]
pub struct RuleDoc {
    pub name: String,
    pub body: String,
    pub path: PathBuf,
    pub needles: Vec<String>,
    pub scope: RuleScope,
    pub gap: RuleGap,
    pub mode: RuleMode,
    pub paths: Vec<yi_permission::PathGlob>,
    pub after: u64,
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

pub fn discover_armed(cwd: &Path, home: &Path) -> RuleSet {
    let mut set = discover(cwd, home);
    let names: BTreeSet<String> = set.rules.iter().map(|rule| rule.name.clone()).collect();
    for compiled in skill_rules(cwd, home) {
        match compiled {
            Err(reason) => set.warnings.push(reason),
            Ok((rule, _)) if names.contains(&rule.name) => {}
            Ok((rule, ignored)) => {
                set.warnings.extend(ignored);
                set.rules.push(rule);
            }
        }
    }
    set
}

fn skill_rules(cwd: &Path, home: &Path) -> Vec<Result<(RuleDoc, Vec<String>), String>> {
    crate::skills::discover(cwd, home)
        .into_iter()
        .map(|skill| {
            skill_as_rule(&skill).map_err(|reason| {
                format!("skill {} skipped as rule: {reason}", skill.path.display())
            })
        })
        .collect()
}

/// Keys that aim a rule at tools drop; only a lost deny is announced, as the model reads notices.
fn skill_as_rule(skill: &crate::skills::Skill) -> Result<(RuleDoc, Vec<String>), String> {
    let mention = format!("${}", skill.name);
    let body = format!("{SKILL_ADDRESS}{}", skill.name);
    let mut fields = skill.frontmatter.clone();
    fields.remove("scope");
    fields.remove("paths");
    let mut ignored = Vec::new();
    if let Some(mode) = fields.remove("mode")
        && mode != "remind"
    {
        ignored.push(format!(
            "skill {}: `mode: {mode}` ignored: a skill only points; a deny belongs in .yi/rules",
            skill.path.display()
        ));
    }
    let mut rule = if fields.contains_key("trigger") {
        fields_to_rule(&skill.path, &fields, &body)?
    } else {
        RuleDoc {
            name: skill.name.clone(),
            body,
            path: skill.path.clone(),
            needles: Vec::new(),
            scope: RuleScope::Text,
            gap: RuleGap::AfterTurns(1),
            mode: RuleMode::Remind,
            paths: Vec::new(),
            after: 1,
        }
    };
    rule.name = skill.name.clone();
    rule.scope = RuleScope::Text;
    if !rule.needles.contains(&mention) {
        rule.needles.push(mention);
    }
    Ok((rule, ignored))
}

pub(crate) fn read_rule(path: &Path) -> Result<RuleDoc, String> {
    let source = std::fs::read_to_string(path).map_err(|error| format!("unreadable: {error}"))?;
    let fields = crate::skills::frontmatter(&source);
    let body = source
        .split_once("---")
        .and_then(|(_, rest)| rest.split_once("---"))
        .map_or(source.as_str(), |(_, body)| body)
        .trim();
    fields_to_rule(path, &fields, body)
}

fn fields_to_rule(
    path: &Path,
    fields: &BTreeMap<String, String>,
    body: &str,
) -> Result<RuleDoc, String> {
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
    let scope = parse_scope(fields.get("scope").map(String::as_str))?;
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
    if mode == RuleMode::Gate
        && matches!(
            scope,
            RuleScope::Text | RuleScope::Result | RuleScope::Error
        )
    {
        return Err(
            "a gate rule needs a tool scope; text/result/error cannot be denied after spawn"
                .to_owned(),
        );
    }
    let paths = parse_paths(fields)?;
    let after = match fields.get("after").map(String::as_str) {
        None => 1,
        Some(raw) => match raw.parse::<u64>() {
            Ok(n) if n > 0 => n,
            _ => return Err(format!("after `{raw}` is not a positive count")),
        },
    };
    if body.is_empty() {
        return Err("empty rule body".to_owned());
    }
    Ok(RuleDoc {
        name,
        body: body.to_owned(),
        path: path.to_path_buf(),
        needles,
        scope,
        gap,
        mode,
        paths,
        after,
    })
}

fn parse_scope(scope: Option<&str>) -> Result<RuleScope, String> {
    match scope {
        None => Ok(RuleScope::AnyTool),
        Some("text") => Ok(RuleScope::Text),
        Some("tool") => Ok(RuleScope::AnyTool),
        Some("result") => Ok(RuleScope::Result),
        Some("error") => Ok(RuleScope::Error),
        Some(other) => match other.strip_prefix("tool:") {
            Some(tool) if !tool.trim().is_empty() => Ok(RuleScope::Tool(tool.trim().to_owned())),
            _ => Err(format!(
                "unknown scope `{other}` (text | tool | tool:<name> | result | error)"
            )),
        },
    }
}

fn parse_paths(fields: &BTreeMap<String, String>) -> Result<Vec<yi_permission::PathGlob>, String> {
    let Some(raw) = fields.get("paths") else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            yi_permission::PathGlob::new(pattern).map_err(|error| format!("paths: {error}"))
        })
        .collect()
}

/// What the rule saw, kept whole rather than hashed: the lines around a needle
/// are not evidence, so `after: N` counts one needle on one path through them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Evidence {
    rule: String,
    needle: String,
    path: String,
}

impl Evidence {
    fn of(rule: &RuleDoc, haystack: &str, path: &str) -> Self {
        Self {
            rule: rule.name.clone(),
            needle: matched_needle(rule, haystack).to_owned(),
            path: path.to_owned(),
        }
    }

    fn skill(rule: &RuleDoc) -> Self {
        Self {
            rule: rule.name.clone(),
            needle: String::new(),
            path: String::new(),
        }
    }
}

#[derive(Default)]
struct FireState {
    turn: u64,
    last_fired: BTreeMap<Evidence, u64>,
    fired_once: BTreeMap<Evidence, bool>,
    seen: BTreeMap<Evidence, u64>,
    loaded: BTreeSet<String>,
}

impl FireState {
    fn eligible(&self, rule: &RuleDoc, evidence: &Evidence) -> bool {
        match rule.gap {
            RuleGap::Once => !self.fired_once.get(evidence).copied().unwrap_or(false),
            RuleGap::AfterTurns(gap) => self
                .last_fired
                .get(evidence)
                .is_none_or(|fired| self.turn.saturating_sub(*fired) >= gap),
        }
    }

    fn mark(&mut self, evidence: &Evidence) {
        self.fired_once.insert(evidence.clone(), true);
        self.last_fired.insert(evidence.clone(), self.turn);
    }

    fn bump(&mut self, evidence: &Evidence) -> u64 {
        let next = self
            .seen
            .get(evidence)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.seen.insert(evidence.clone(), next);
        next
    }

    fn rearm(&mut self) {
        self.fired_once.clear();
        self.last_fired.clear();
        self.seen.clear();
        self.loaded.clear();
    }
}

/// Literal substring matcher: a user's rule reads tool text and prose, a skill only typed input.
pub struct RuleEngine {
    rules: std::sync::RwLock<Vec<RuleDoc>>,
    state: Mutex<FireState>,
    deliver: Mutex<Option<DeliverFn>>,
    fetch: Mutex<Option<Arc<FetchLog>>>,
    classifier: Mutex<Option<Arc<crate::classifier::SkillClassifier>>>,
}

fn matches(rule: &RuleDoc, haystack: &str) -> bool {
    rule.needles.iter().any(|needle| haystack.contains(needle))
}

fn matched_needle<'a>(rule: &'a RuleDoc, haystack: &str) -> &'a str {
    rule.needles
        .iter()
        .find(|needle| haystack.contains(needle.as_str()))
        .map(String::as_str)
        .unwrap_or("")
}

fn tool_gate_scope(rule: &RuleDoc, tool: &str) -> bool {
    match &rule.scope {
        RuleScope::Text | RuleScope::Result | RuleScope::Error => false,
        RuleScope::AnyTool => true,
        RuleScope::Tool(name) => name == tool,
    }
}

fn result_scope(rule: &RuleDoc, is_error: bool) -> bool {
    match rule.scope {
        RuleScope::Result => true,
        RuleScope::Error => is_error,
        RuleScope::Text | RuleScope::AnyTool | RuleScope::Tool(_) => false,
    }
}

fn call_path(args_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args_json)
        .ok()
        .and_then(|value| {
            value
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// `paths:` reads the call's `path` argument alone: a rule with one skips `bash`.
fn paths_ok(rule: &RuleDoc, path: &str) -> bool {
    if rule.paths.is_empty() {
        return true;
    }
    !path.is_empty() && rule.paths.iter().any(|glob| glob.is_match(path))
}

fn skill_name(rule: &RuleDoc) -> Option<&str> {
    rule.body.strip_prefix(SKILL_ADDRESS)
}

/// ASCII case folds on both sides: a capital at the start of a sentence is the same request.
fn typed_needle<'a>(rule: &'a RuleDoc, folded: &str) -> Option<&'a str> {
    rule.needles
        .iter()
        .find(|needle| {
            let needle = needle.to_ascii_lowercase();
            if needle.strip_prefix('$') == Some(&rule.name.to_ascii_lowercase()) {
                mentions(folded, &needle)
            } else {
                folded.contains(&needle)
            }
        })
        .map(String::as_str)
}

fn mentions(folded: &str, mention: &str) -> bool {
    folded
        .split(mention)
        .skip(1)
        .any(|after| !after.starts_with(name_char))
}

fn name_char(next: char) -> bool {
    next.is_ascii_alphanumeric() || next == '-' || next == '_'
}

fn typed_text(message: &AgentMessage) -> Option<String> {
    let AgentMessage::User {
        content,
        attribution: Attribution::User,
        ..
    } = message
    else {
        return None;
    };
    Some(match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => yi_types::message::join_text(blocks, "\n"),
    })
}

pub(crate) fn reminder(text: String) -> AgentMessage {
    AgentMessage::host_note("reminder", text, yi_session::now_ms())
}

impl RuleEngine {
    pub fn new(rules: Vec<RuleDoc>) -> Self {
        Self {
            rules: std::sync::RwLock::new(rules),
            state: Mutex::new(FireState::default()),
            deliver: Mutex::new(None),
            fetch: Mutex::new(None),
            classifier: Mutex::new(None),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.snapshot().is_empty()
    }

    fn snapshot(&self) -> Vec<RuleDoc> {
        self.rules
            .read()
            .map(|rules| rules.clone())
            .unwrap_or_default()
    }

    /// Invariant: one name, one rule; a re-promotion replaces rather than stacks.
    pub fn insert(&self, rule: RuleDoc) {
        if let Ok(mut rules) = self.rules.write() {
            rules.retain(|existing| existing.name != rule.name);
            rules.push(rule);
        }
    }

    pub fn set_deliver(&self, deliver: DeliverFn) {
        if let Ok(mut slot) = self.deliver.lock() {
            *slot = Some(deliver);
        }
    }

    pub fn set_classifier(&self, classifier: Arc<crate::classifier::SkillClassifier>) {
        if let Ok(mut slot) = self.classifier.lock() {
            *slot = Some(classifier);
        }
    }

    pub fn set_fetch(&self, log: Arc<FetchLog>) {
        if let Ok(mut slot) = self.fetch.lock() {
            *slot = Some(log);
        }
    }

    /// Incident: `reminder` is dropped at compaction while `fired_once` survived, so `gap: once` stayed spent.
    pub fn rearm(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.rearm();
        }
        if let Ok(slot) = self.classifier.lock()
            && let Some(classifier) = slot.as_ref()
        {
            classifier.rearm();
        }
    }

    pub fn check_tool(&self, tool: &str, args_json: &str) -> Option<String> {
        self.scan(tool, args_json, args_json, false, true)
    }

    pub fn check_result(&self, tool: &str, args_json: &str, result: &str, is_error: bool) {
        self.note_skill_read(tool, args_json);
        // The tool already ran: a post-spawn scan only reminds, so its denial slot is always None.
        let _ = self.scan(tool, args_json, result, is_error, false);
    }

    fn note_skill_read(&self, tool: &str, args_json: &str) {
        if tool != "read" {
            return;
        }
        let path = call_path(args_json);
        if path.is_empty() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for rule in self.snapshot() {
            let Some(name) = skill_name(&rule) else {
                continue;
            };
            let suffix = format!("{name}/SKILL.md");
            if path.ends_with(&suffix) {
                state.loaded.insert(name.to_owned());
            }
        }
    }

    fn scan(
        &self,
        tool: &str,
        args_json: &str,
        haystack: &str,
        is_error: bool,
        pre: bool,
    ) -> Option<String> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let mut denial = None;
        let mut reminders = Vec::new();
        let path = call_path(args_json);
        let rules = self.snapshot();
        for rule in &rules {
            let in_scope = if pre {
                tool_gate_scope(rule, tool)
            } else {
                result_scope(rule, is_error)
            };
            if skill_name(rule).is_some()
                || !in_scope
                || !paths_ok(rule, &path)
                || !matches(rule, haystack)
            {
                continue;
            }
            let evidence = Evidence::of(rule, haystack, &path);
            let n = state.bump(&evidence);
            if n < rule.after.max(1) || !state.eligible(rule, &evidence) {
                continue;
            }
            match rule.mode {
                RuleMode::Gate if pre && denial.is_none() => {
                    state.mark(&evidence);
                    denial = Some(format!(
                        "Denied by rule `{}` ({}):\n{}",
                        rule.name,
                        rule.path.display(),
                        rule.body
                    ));
                }
                RuleMode::Gate => {}
                RuleMode::Remind => {
                    state.mark(&evidence);
                    reminders.push(self.render_reminder(rule, &evidence.needle));
                }
            }
        }
        drop(state);
        self.deliver_reminders(reminders);
        denial
    }

    // Incident: `read` once bypassed FetchLog; `loaded` is the suppress beside the logged skill reads.
    fn skill_already_loaded(&self, state: &FireState, rule: &RuleDoc) -> bool {
        let Some(name) = skill_name(rule) else {
            return false;
        };
        if state.loaded.contains(name) {
            return true;
        }
        let Ok(slot) = self.fetch.lock() else {
            return false;
        };
        let Some(log) = slot.as_ref() else {
            return false;
        };
        let needle = format!("{SKILL_ADDRESS}{name}");
        log.records().iter().any(|record| record.url == needle)
    }

    fn render_reminder(&self, rule: &RuleDoc, needle: &str) -> String {
        if let Some(name) = skill_name(rule) {
            format!("Relevant: {SKILL_ADDRESS}{name} (matched \"{needle}\")")
        } else {
            format!("<rule name=\"{}\">\n{}\n</rule>", rule.name, rule.body)
        }
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
        // Incident: the advisor's session-scoped dedupe would override a user-chosen re-arm gap.
        for text in reminders {
            deliver(reminder(text), DeliveryMode::Steer);
        }
    }

    /// Only a message the user typed points at a skill; the pointers are placed right behind it.
    pub fn observe_user(&self, message: &AgentMessage) -> Vec<AgentMessage> {
        let Some(text) = typed_text(message) else {
            return Vec::new();
        };
        let folded = text.to_ascii_lowercase();
        let rules = self.snapshot();
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let mut texts = Vec::new();
        let classifier = self.classifier.lock().ok().and_then(|slot| slot.clone());
        let mut pointed: Vec<String> = rules
            .iter()
            .filter(|rule| self.skill_already_loaded(&state, rule))
            .filter_map(skill_name)
            .map(str::to_owned)
            .collect();
        let mut counted = 0_usize;
        let mut dropped = Vec::new();
        for rule in &rules {
            let Some(name) = skill_name(rule) else {
                continue;
            };
            let Some(needle) = typed_needle(rule, &folded) else {
                continue;
            };
            let mention = format!("${name}");
            let named = mentions(&folded, &mention.to_ascii_lowercase());
            let evidence = Evidence::skill(rule);
            let n = state.bump(&evidence);
            if !named && (n < rule.after.max(1) || !state.eligible(rule, &evidence)) {
                continue;
            }
            if self.skill_already_loaded(&state, rule) {
                continue;
            }
            if !named && classifier.as_ref().is_some_and(|c| c.has_pointed(name)) {
                continue;
            }
            if !named {
                // A dropped pointer is not latched, so the next message that matches it delivers.
                if counted >= POINTER_CAP {
                    dropped.push(format!("{SKILL_ADDRESS}{name}"));
                    continue;
                }
                if classifier.as_ref().is_some_and(|c| !c.claim(name)) {
                    continue;
                }
                counted += 1;
            } else if let Some(classifier) = &classifier {
                classifier.claim(name);
            }
            state.mark(&evidence);
            pointed.push(name.to_owned());
            let shown = if named { mention.as_str() } else { needle };
            texts.push(self.render_reminder(rule, shown));
        }
        drop(state);
        if let Some(classifier) = classifier {
            classifier.consult(&text, pointed);
        }
        if let Some(last) = texts.last_mut()
            && !dropped.is_empty()
        {
            last.push_str(&format!(
                " [+{} past the cap of {POINTER_CAP}: {}]",
                dropped.len(),
                dropped.join(", ")
            ));
        }
        texts.into_iter().map(reminder).collect()
    }

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
        let text: String = yi_types::message::join_text(content, "\n");
        let mut reminders = Vec::new();
        let rules = self.snapshot();
        if let Ok(mut state) = self.state.lock() {
            if !text.is_empty() {
                for rule in &rules {
                    if rule.mode != RuleMode::Remind
                        || rule.scope != RuleScope::Text
                        || skill_name(rule).is_some()
                        || !matches(rule, &text)
                    {
                        continue;
                    }
                    let evidence = Evidence::of(rule, &text, "");
                    let n = state.bump(&evidence);
                    if n < rule.after.max(1) || !state.eligible(rule, &evidence) {
                        continue;
                    }
                    state.mark(&evidence);
                    reminders.push(self.render_reminder(rule, &evidence.needle));
                }
            }
            state.turn = state.turn.saturating_add(1);
        }
        self.deliver_reminders(reminders);
    }
}

/// Discovery + observer wiring; the tool gate half rides [`crate::tools::ToolAdapter`].
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
