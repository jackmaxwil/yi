use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use yi_types::classifier::{ClassifyRecord, DecisionRequest, DecisionResponse, Question};
use yi_types::config::{ApprovalMode, ClassifierConfig, UserConfig};
use yi_types::message::{AgentMessage, Content, UserContent};

use crate::AgentSession;
use crate::mailbox::clamp;

pub const DEFAULT_URL: &str = "https://openrouter.ai/api";
pub const JEV: &str = "typesafe/jev-1.13";
pub const NOTICE_TYPE: &str = "classifier";
const DEFAULT_TIMEOUT_MS: u64 = 1000;
const CANDIDATE_CAP: usize = 20;
const CRITERION_CHARS: usize = 60;
const TRIP_AFTER: u32 = 3;
const PAUSE: Duration = Duration::from_secs(60);
const QUESTION: &str = "skill";
const NONE: &str = "none";
const INSTRUCTIONS: &str =
    "Which of these methods does the user's message ask for? Answer none unless one clearly does.";

pub type Record = Arc<dyn Fn(ClassifyRecord) + Send + Sync>;
pub use crate::Deliver;

#[derive(Clone)]
pub struct Sidecar {
    pub url: String,
    pub key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub threshold: f64,
}

impl Sidecar {
    fn named(&self, error: &str) -> String {
        format!("{}: {error}", self.url)
    }
}

#[derive(Default)]
struct Breaker {
    failures: u32,
    paused_until: Option<Instant>,
}

fn paused(breaker: &Mutex<Breaker>) -> bool {
    let Ok(mut breaker) = breaker.lock() else {
        return true;
    };
    match breaker.paused_until {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            breaker.paused_until = None;
            false
        }
        None => false,
    }
}

fn fail(breaker: &Mutex<Breaker>) -> bool {
    let Ok(mut breaker) = breaker.lock() else {
        return false;
    };
    breaker.failures = breaker.failures.saturating_add(1);
    if breaker.failures < TRIP_AFTER {
        return false;
    }
    breaker.failures = 0;
    breaker.paused_until = Instant::now().checked_add(PAUSE);
    true
}

fn succeed(breaker: &Mutex<Breaker>) {
    if let Ok(mut breaker) = breaker.lock() {
        breaker.failures = 0;
    }
}

pub struct SkillClassifier {
    sidecar: Sidecar,
    candidates: BTreeMap<String, String>,
    breaker: Mutex<Breaker>,
    record: Record,
    deliver: Deliver,
    pointed: Mutex<BTreeMap<String, bool>>,
}

impl SkillClassifier {
    pub fn new(
        sidecar: Sidecar,
        skills: Vec<(String, String)>,
        record: Record,
        deliver: Deliver,
    ) -> Result<Self, String> {
        if skills.len() > CANDIDATE_CAP {
            return Err(format!(
                "{} skills declare a trigger and the classifier asks about at most {CANDIDATE_CAP}, so it stays off",
                skills.len()
            ));
        }
        let candidates = skills
            .into_iter()
            .map(|(name, description)| (name, description.chars().take(CRITERION_CHARS).collect()))
            .collect();
        Ok(Self {
            sidecar,
            candidates,
            breaker: Mutex::new(Breaker::default()),
            record,
            deliver,
            pointed: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn has_pointed(&self, name: &str) -> bool {
        self.pointed
            .lock()
            .is_ok_and(|pointed| pointed.get(name) == Some(&true))
    }

    pub fn claim(&self, name: &str) -> bool {
        self.pointed
            .lock()
            .is_ok_and(|mut pointed| !*pointed.entry(name.to_owned()).or_insert(false))
    }

    pub fn rearm(&self) {
        if let Ok(mut pointed) = self.pointed.lock() {
            pointed.clear();
        }
    }

    pub fn consult(self: &Arc<Self>, text: &str, already: Vec<String>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.candidates.is_empty() || paused(&self.breaker) {
            return;
        }
        let this = Arc::clone(self);
        let request = self.request(text);
        let message = message_id(text);
        runtime.spawn(async move {
            let started = Instant::now();
            let (url, key, deadline) = (
                this.sidecar.url.clone(),
                this.sidecar.key.clone(),
                this.sidecar.timeout,
            );
            let outcome = tokio::task::spawn_blocking(move || {
                yi_ai::decide::decide(&url, key.as_deref(), deadline, &request)
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            this.settle(outcome, latency_ms, message, &already);
        });
    }

    fn request(&self, text: &str) -> DecisionRequest {
        let mut criteria = self.candidates.clone();
        criteria.insert(NONE.to_owned(), "no listed method applies".to_owned());
        DecisionRequest {
            state: [("message".to_owned(), serde_json::json!(text))]
                .into_iter()
                .collect(),
            questions: [(
                QUESTION.to_owned(),
                Question::Choice {
                    instructions: INSTRUCTIONS.to_owned(),
                    criteria,
                },
            )]
            .into(),
            model: Some(self.sidecar.model.clone()),
        }
    }

    fn settle(
        &self,
        outcome: Result<DecisionResponse, String>,
        latency_ms: u64,
        message: String,
        already: &[String],
    ) {
        let mut record = ClassifyRecord {
            consumer: QUESTION.to_owned(),
            message,
            answer: None,
            confidence: None,
            model: None,
            latency_ms,
            fired: false,
            error: None,
            extra: BTreeMap::new(),
        };
        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                if fail(&self.breaker) {
                    (self.deliver)(self.notice(&error));
                }
                record.error = Some(self.sidecar.named(&error));
                (self.record)(record);
                return;
            }
        };
        succeed(&self.breaker);
        let answer = response.answers.get(QUESTION);
        record.answer = answer.and_then(|answer| answer.choice.clone());
        record.confidence = answer.and_then(|answer| {
            answer.answer_confidence.or_else(|| {
                let chosen = answer
                    .probabilities
                    .as_ref()?
                    .get(answer.choice.as_deref()?)?;
                chosen.as_f64()
            })
        });
        record.model = (response.routing.map(|routing| routing.model)).or(response.model);
        if let (Some(name), Some(confidence)) = (record.answer.as_deref(), record.confidence)
            && self.sidecar.threshold < 1.0
            && confidence >= self.sidecar.threshold
            && self.candidates.contains_key(name)
            && !already.iter().any(|pointed| pointed == name)
            && self.pointed.lock().is_ok_and(|mut pointed| {
                !pointed.contains_key(name) && pointed.insert(name.to_owned(), true).is_none()
            })
        {
            (self.deliver)(crate::rules::reminder(format!(
                "Relevant: {}{name} (the classifier, {confidence:.2})",
                crate::rules::SKILL_ADDRESS
            )));
            record.fired = true;
        }
        (self.record)(record);
    }

    fn notice(&self, error: &str) -> AgentMessage {
        AgentMessage::Custom {
            custom_type: NOTICE_TYPE.to_owned(),
            content: UserContent::Text(format!(
                "The classifier at {} failed {TRIP_AFTER} times in a row (last: {error}); skill reminders use trigger words only for the next {} s.",
                self.sidecar.url,
                PAUSE.as_secs()
            )),
            display: true,
            details: None,
            timestamp: yi_session::now_ms(),
        }
    }
}

const SAFE: &str = "safe";
const RUBRIC: &str = "Is it safe for a coding agent to run this without asking the user? Safe means it cannot destroy work a checkpoint would not restore, leak a secret, or act outside the project. Unsafe: deleting or overwriting anything outside the project root or inside .git; sending any file, environment variable or credential to a remote host; running downloaded code; force-pushing, publishing or deleting remote resources; changing system or permission settings; anything the user's task rules out. Safe: reading, building, testing, formatting, editing or deleting files inside the project, local git operations a checkpoint can undo, and the push the user's task asks for.";
const CHECKPOINTS: &str = "Yi snapshots the working tree before every turn: files changed or deleted inside the project restore; .git, files outside the project and remote systems do not";
const DEFAULT_THRESHOLD: f64 = 0.9;
const DEFAULT_ALLOW_AT: f64 = 0.8;
const DEFAULT_ALLOW_DESTRUCTIVE_AT: f64 = 0.9;
const DEFAULT_ASK_AT: f64 = 0.05;
const DEFAULT_DELAY_SECS: u64 = 30;
const CONTEXT_CHARS: usize = 1000;
const CONTEXT_NAME: &str = "classifier.context_chars";
const CONTEXT_NEXT: &str = "the classifier judges this head only";
const COMMAND_CHARS: usize = 4000;
const WRITABLE_CAP: usize = 8;
const RECENT_CALLS: usize = 2;
const HISTORY_TAIL: usize = 256;
const PROBE_MS: u64 = 3000;

pub struct Thresholds {
    pub allow_at: f64,
    pub allow_destructive_at: f64,
    pub ask_at: f64,
}

pub struct Call<'a> {
    pub tool: &'a str,
    pub display: &'a str,
    pub command: Option<&'a str>,
    pub reason: &'a str,
    pub cwd: &'a str,
}

/// The project facts the approval question carries beside the call.
pub struct Project {
    pub root: String,
    pub writable: Vec<String>,
    pub protected_branches: Vec<String>,
}

pub type History = Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>;

struct Context {
    project: Project,
    history: History,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Judgement {
    Allow(f64),
    AskUser(f64),
    Undecided,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    Instant,
    AfterDelay(Duration),
}

pub struct Approver {
    sidecar: Sidecar,
    thresholds: Thresholds,
    timing: Timing,
    breaker: Mutex<Breaker>,
    record: Record,
    context: Option<Context>,
}

impl Approver {
    pub fn new(sidecar: Sidecar, thresholds: Thresholds, timing: Timing, record: Record) -> Self {
        Self {
            sidecar,
            thresholds,
            timing,
            breaker: Mutex::new(Breaker::default()),
            record,
            context: None,
        }
    }

    /// The user's task and recent calls come from `history`, read at each judgement.
    pub fn with_context(mut self, project: Project, history: History) -> Self {
        self.context = Some(Context { project, history });
        self
    }

    pub fn timing(&self) -> Timing {
        self.timing
    }

    pub fn judge(&self, call: &Call<'_>) -> Judgement {
        if paused(&self.breaker) {
            return Judgement::Undecided;
        }
        let class = class(call.command);
        let mut record = ClassifyRecord {
            consumer: "approve".to_owned(),
            message: message_id(call.display),
            answer: None,
            confidence: None,
            model: None,
            latency_ms: 0,
            fired: false,
            error: None,
            extra: [("verdictClass".to_owned(), serde_json::json!(class))].into(),
        };
        let started = Instant::now();
        let length = call.display.chars().count();
        let safe = if length > COMMAND_CHARS {
            record.error = Some(format!(
                "a {length}-char command is past the {COMMAND_CHARS}-char cap (COMMAND_CHARS) on what the classifier judges, so the person decides"
            ));
            None
        } else {
            let request = DecisionRequest {
                state: self.state(call),
                questions: [(
                    SAFE.to_owned(),
                    Question::Noul {
                        instructions: RUBRIC.to_owned(),
                    },
                )]
                .into(),
                model: Some(self.sidecar.model.clone()),
            };
            match yi_ai::decide::decide(
                &self.sidecar.url,
                self.sidecar.key.as_deref(),
                self.sidecar.timeout,
                &request,
            ) {
                Ok(response) => {
                    succeed(&self.breaker);
                    record.model =
                        (response.routing.map(|routing| routing.model)).or(response.model);
                    response.answers.get(SAFE).and_then(|answer| answer.noul)
                }
                Err(error) => {
                    fail(&self.breaker);
                    record.error = Some(self.sidecar.named(&error));
                    None
                }
            }
        };
        record.latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let allow_at = if class != "ordinary" {
            self.thresholds.allow_destructive_at
        } else {
            self.thresholds.allow_at
        };
        let judgement = match safe {
            Some(p) if p >= allow_at => Judgement::Allow(p),
            Some(p) if p <= self.thresholds.ask_at => Judgement::AskUser(p),
            _ => Judgement::Undecided,
        };
        record.confidence = safe;
        record.answer = Some(
            match judgement {
                Judgement::Allow(_) => "allow",
                Judgement::AskUser(_) => "ask",
                Judgement::Undecided => "undecided",
            }
            .to_owned(),
        );
        record.fired = matches!(judgement, Judgement::Allow(_));
        (self.record)(record);
        judgement
    }

    fn state(&self, call: &Call<'_>) -> serde_json::Map<String, serde_json::Value> {
        let why = (call.reason.strip_suffix(call.display))
            .and_then(|reason| reason.strip_suffix(": "))
            .unwrap_or(call.reason);
        let mut state: serde_json::Map<_, _> = [
            (
                "tool",
                clamp(call.tool, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT),
            ),
            ("command", call.display.to_owned()),
            (
                "cwd",
                clamp(call.cwd, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT),
            ),
            (
                "why Yi asks",
                clamp(why, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), serde_json::json!(value)))
        .collect();
        let Some(Context { project, history }) = &self.context else {
            return state;
        };
        let messages = history();
        if let Some(task) = messages.iter().rev().find_map(crate::rules::typed_text) {
            state.insert(
                "user task".to_owned(),
                serde_json::json!(clamp(&task, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT)),
            );
        }
        state.insert(
            "recent commands".to_owned(),
            serde_json::json!(recent(&messages)),
        );
        let mut writable: Vec<String> = (project.writable.iter())
            .take(WRITABLE_CAP)
            .map(|root| clamp(root, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT))
            .collect();
        if project.writable.len() > WRITABLE_CAP {
            writable.push(format!(
                "… [kept {WRITABLE_CAP} of {} writable roots · cap WRITABLE_CAP = {WRITABLE_CAP} · the root above is the one sent]",
                project.writable.len()
            ));
        }
        state.insert(
            "project".to_owned(),
            serde_json::json!({
                "root": clamp(&project.root, CONTEXT_CHARS, CONTEXT_NAME, CONTEXT_NEXT),
                "writable": writable,
                "protected branches": project.protected_branches,
                "checkpoints": CHECKPOINTS,
            }),
        );
        state
    }
}

/// The last calls that have a result, oldest first: the call being judged has none yet.
fn recent(messages: &[AgentMessage]) -> Vec<String> {
    let answered: std::collections::BTreeSet<&str> = (messages.iter())
        .filter_map(|message| match message {
            AgentMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    let mut calls: Vec<String> = (messages.iter().rev())
        .filter_map(|message| match message {
            AgentMessage::Assistant { content, .. } => Some(content.iter().rev()),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } if answered.contains(id.as_str()) => Some(clamp(
                &call_line(name, arguments),
                CONTEXT_CHARS,
                CONTEXT_NAME,
                CONTEXT_NEXT,
            )),
            _ => None,
        })
        .take(RECENT_CALLS)
        .collect();
    calls.reverse();
    calls
}

/// Incident: a `write` or `edit` call's arguments are the file's body, so a judgement reads the
/// tool and the path it names, never the payload it carries.
fn call_line(name: &str, arguments: &serde_json::Map<String, serde_json::Value>) -> String {
    let text = |key: &str| arguments.get(key).and_then(serde_json::Value::as_str);
    if name == "bash" {
        return text("command").unwrap_or(name).to_owned();
    }
    match text("path").or_else(|| text("file_path")) {
        Some(path) => format!("{name} {path}"),
        None => name.to_owned(),
    }
}

fn class(command: Option<&str>) -> &'static str {
    let Some(yi_permission::Parsed::Segments(segments)) = command.map(yi_permission::parse) else {
        return "unproven";
    };
    let classes: Vec<_> = segments
        .iter()
        .map(|argv| yi_permission::classify(argv))
        .collect();
    if classes.contains(&yi_permission::Class::Destructive) {
        "destructive"
    } else if classes.contains(&yi_permission::Class::Egress) {
        "egress"
    } else if segments.iter().flatten().any(|word| word.starts_with('#')) {
        "unproven"
    } else {
        "ordinary"
    }
}

pub fn message_id(text: &str) -> String {
    let normal = text
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    Sha256::digest(normal.as_bytes())
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct Endpoint {
    pub model: String,
    pub url: String,
}

pub enum Armed {
    Off,
    At(Endpoint),
    /// A config that names a classifier which will not run, and why.
    Refused(String),
}

/// Incident: a config may still name the retired Laya checkpoint, whose loopback URL no sidecar
/// answers any more, and the bearer is the user's own OpenRouter account key.
pub fn arm(config: &UserConfig) -> Armed {
    let Some(named) = (config.models.as_ref()).and_then(|models| models.classifier.clone()) else {
        return Armed::Off;
    };
    if !named.contains('/') {
        return Armed::Refused(format!(
            "models.classifier `{named}` is a retired Laya checkpoint and no longer runs, so the classifier stays off; `{JEV}` is its replacement at OpenRouter"
        ));
    }
    let Some(configured) = (config.classifier.as_ref()).and_then(|block| block.url.clone()) else {
        return Armed::At(Endpoint {
            model: named,
            url: DEFAULT_URL.to_owned(),
        });
    };
    if !reachable(&configured) {
        return Armed::Refused(format!(
            "classifier.url {configured} is neither {DEFAULT_URL} nor loopback, so the classifier stays off rather than send the OpenRouter key there"
        ));
    }
    Armed::At(Endpoint {
        model: named,
        url: configured,
    })
}

/// Invariant: a configured URL is honoured only where the account key may go, which is the
/// provider's own host over TLS and this machine.
fn reachable(url: &str) -> bool {
    let Some(named) = host(url) else {
        return false;
    };
    (url.starts_with("https://") && host(DEFAULT_URL) == Some(named))
        || matches!(named, "127.0.0.1" | "localhost" | "::1")
}

fn host(url: &str) -> Option<&str> {
    let rest = (url.strip_prefix("https://")).or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?;
    match host.strip_prefix('[') {
        Some(literal) => literal.split(']').next(),
        None => Some(host.split(':').next()?),
    }
}

/// One question with a short deadline: does the endpoint answer at all, or is a row green over a
/// key that was revoked? Answers the round trip in milliseconds.
pub fn probe(at: &Endpoint, key: Option<&str>) -> Result<u64, String> {
    let request = DecisionRequest {
        state: [("message".to_owned(), serde_json::json!("ping"))]
            .into_iter()
            .collect(),
        questions: [(
            "probe".to_owned(),
            Question::Noul {
                instructions: "Answer yes.".to_owned(),
            },
        )]
        .into(),
        model: Some(at.model.clone()),
    };
    let started = Instant::now();
    yi_ai::decide::decide(&at.url, key, Duration::from_millis(PROBE_MS), &request)?;
    Ok(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
}

pub fn timing(block: &ClassifierConfig) -> Option<Timing> {
    let mode = match (block.approval, block.approve, block.ask_timeout_secs) {
        (Some(mode), _, _) => mode,
        (None, Some(false), _) => ApprovalMode::WaitForUser,
        (None, _, _) => ApprovalMode::Instant,
    };
    match mode {
        ApprovalMode::Instant => Some(Timing::Instant),
        ApprovalMode::AfterDelay => match block.ask_timeout_secs.unwrap_or(DEFAULT_DELAY_SECS) {
            0 => None,
            secs => Some(Timing::AfterDelay(Duration::from_secs(secs))),
        },
        ApprovalMode::WaitForUser => None,
    }
}

fn number(value: &Option<serde_json::Number>, default: f64) -> f64 {
    value
        .as_ref()
        .and_then(serde_json::Number::as_f64)
        .unwrap_or(default)
}

fn threshold(block: &ClassifierConfig) -> f64 {
    number(&block.threshold, DEFAULT_THRESHOLD)
}

pub fn thresholds(block: &ClassifierConfig) -> Thresholds {
    Thresholds {
        allow_at: number(&block.allow_at, DEFAULT_ALLOW_AT),
        allow_destructive_at: number(&block.allow_destructive_at, DEFAULT_ALLOW_DESTRUCTIVE_AT),
        ask_at: number(&block.ask_at, DEFAULT_ASK_AT),
    }
}

pub fn attach(session: &AgentSession, cwd: &Path, home: &Path, config: &UserConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    let Endpoint { model, url } = match arm(config) {
        Armed::Off => return warnings,
        Armed::At(at) => at,
        Armed::Refused(why) => {
            warnings.push(why);
            return warnings;
        }
    };
    let block = config.classifier.clone().unwrap_or_default();
    let timing = timing(&block);
    let broker = session.permission_broker();
    if let (Some(Timing::AfterDelay(_)), Some(broker)) = (&timing, broker.as_ref())
        && !broker.asks_close_on_settle()
    {
        warnings.push(
            "classifier.approval is after-delay, but this surface's prompts never close on \
             settle, so approval acts as wait-for-user here"
                .to_owned(),
        );
    }
    let Some(key) = yi_ai::auth::stored_key(home, "openrouter") else {
        warnings.push(format!(
            "{}, so the classifier stays off",
            yi_ai::auth::missing_message("openrouter")
        ));
        return warnings;
    };
    let sidecar = Sidecar {
        url,
        key: Some(key.expose().to_owned()),
        model,
        timeout: Duration::from_millis(block.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
        threshold: threshold(&block),
    };
    if let (Some(timing), Some(broker)) = (timing, broker) {
        let writable = match broker.sandbox() {
            Some(sandbox) => (sandbox.writable.iter())
                .map(|root| root.display().to_string())
                .collect(),
            None => vec![cwd.display().to_string()],
        };
        let project = Project {
            root: cwd.display().to_string(),
            writable,
            protected_branches: vec!["main".to_owned()],
        };
        let approver = Approver::new(
            sidecar.clone(),
            thresholds(&block),
            timing,
            journal(session),
        );
        broker.set_approver(Arc::new(
            approver.with_context(project, session.history_tail(HISTORY_TAIL)),
        ));
    }
    let Some(rules) = session.rules_engine() else {
        return warnings;
    };
    let skills = crate::skills::discover(cwd, home)
        .into_iter()
        .filter(|skill| skill.frontmatter.contains_key("trigger"))
        .map(|skill| (skill.name, skill.description))
        .collect();
    let queue = session.deliver_hook();
    let deliver: Deliver = Arc::new(move |message| queue(message, false));
    match SkillClassifier::new(sidecar, skills, journal(session), deliver) {
        Ok(classifier) => rules.set_classifier(Arc::new(classifier)),
        Err(why) => warnings.push(why),
    }
    warnings
}

fn journal(session: &AgentSession) -> Record {
    let store = session.store_handle();
    Arc::new(move |record| {
        if let Some(store) = store() {
            let _journaled = yi_session::lock_session(&store).append_custom_record(&record);
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_retired_laya_checkpoint_stays_off_and_a_named_model_keeps_a_url_it_may_reach()
    -> serde_json::Result<()> {
        let (owners, _) = yi_types::config::parse(
            r#"{"models":{"classifier":"english"},"classifier":{"url":"http://127.0.0.1:8000","threshold":0.9}}"#,
        )?;
        assert!(
            matches!(super::arm(&owners), super::Armed::Refused(why) if why.contains("english")),
            "the Laya era config must not be repointed at a paid remote model"
        );
        let (own, _) = yi_types::config::parse(
            r#"{"models":{"classifier":"typesafe/jev-1.13"},"classifier":{"url":"http://127.0.0.1:9"}}"#,
        )?;
        let at = match super::arm(&own) {
            super::Armed::At(at) => at,
            _ => {
                return Err(serde_json::Error::io(std::io::Error::other(
                    "arm refused loopback",
                )));
            }
        };
        assert_eq!(at.url, "http://127.0.0.1:9");
        let (lan, _) = yi_types::config::parse(
            r#"{"models":{"classifier":"typesafe/jev-1.13"},"classifier":{"url":"http://not-openrouter.example:8000"}}"#,
        )?;
        assert!(
            matches!(super::arm(&lan), super::Armed::Refused(why) if why.contains("not-openrouter.example")),
            "the account key must not reach a host off this machine"
        );
        let (plain, _) = yi_types::config::parse(
            r#"{"models":{"classifier":"typesafe/jev-1.13"},"classifier":{"url":"http://openrouter.ai/api"}}"#,
        )?;
        assert!(matches!(super::arm(&plain), super::Armed::Refused(_)));
        Ok(())
    }

    #[test]
    fn an_unset_threshold_is_0_9_and_a_set_one_wins() -> serde_json::Result<()> {
        let read = |json: serde_json::Value| {
            serde_json::from_value::<yi_types::config::ClassifierConfig>(json)
        };
        assert_eq!(super::threshold(&read(serde_json::json!({}))?), 0.9);
        assert_eq!(
            super::threshold(&read(serde_json::json!({"threshold": 0.5}))?),
            0.5
        );
        Ok(())
    }
}
