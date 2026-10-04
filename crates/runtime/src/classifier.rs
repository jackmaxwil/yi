use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use yi_types::classifier::{ClassifyRecord, DecisionRequest, DecisionResponse, Question};
use yi_types::config::{ApprovalMode, ClassifierConfig, UserConfig};
use yi_types::message::{AgentMessage, UserContent};

use crate::AgentSession;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8000";
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
    pub threshold: Option<f64>,
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
            state: [("message".to_owned(), serde_json::json!(text))].into(),
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
        record.confidence = answer.and_then(|answer| answer.answer_confidence);
        record.model = response.routing.map(|routing| routing.model);
        if let (Some(name), Some(confidence), Some(threshold)) = (
            record.answer.as_deref(),
            record.confidence,
            self.sidecar.threshold,
        ) && confidence >= threshold
            && self.candidates.contains_key(name)
            && !already.iter().any(|pointed| pointed == name)
            && self.pointed.lock().is_ok_and(|mut pointed| {
                !pointed.contains_key(name) && pointed.insert(name.to_owned(), true).is_none()
            })
        {
            (self.deliver)(crate::rules::reminder(format!(
                "Relevant: skill://{name} (the classifier, {confidence:.2})"
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
const SAFE_INSTRUCTIONS: &str = "Is it safe for a coding agent to run this without asking the user? Safe means it cannot destroy work a checkpoint would not restore, leak a secret, or act outside the project.";
const DEFAULT_ALLOW_AT: f64 = 0.9;
const DEFAULT_ALLOW_DESTRUCTIVE_AT: f64 = 0.98;
const DEFAULT_ASK_AT: f64 = 0.05;
const DEFAULT_DELAY_SECS: u64 = 30;

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
}

impl Approver {
    pub fn new(sidecar: Sidecar, thresholds: Thresholds, timing: Timing, record: Record) -> Self {
        Self {
            sidecar,
            thresholds,
            timing,
            breaker: Mutex::new(Breaker::default()),
            record,
        }
    }

    pub fn timing(&self) -> Timing {
        self.timing
    }

    pub fn judge(&self, call: &Call<'_>) -> Judgement {
        if paused(&self.breaker) {
            return Judgement::Undecided;
        }
        let class = class(call.command);
        let state = [
            ("tool", call.tool),
            ("command", call.display),
            ("cwd", call.cwd),
            (
                "why Yi asks",
                call.reason
                    .strip_suffix(call.display)
                    .and_then(|reason| reason.strip_suffix(": "))
                    .unwrap_or(call.reason),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), serde_json::json!(value)))
        .collect();
        let request = DecisionRequest {
            state,
            questions: [(
                SAFE.to_owned(),
                Question::Noul {
                    instructions: SAFE_INSTRUCTIONS.to_owned(),
                },
            )]
            .into(),
            model: Some(self.sidecar.model.clone()),
        };
        let started = Instant::now();
        let outcome = yi_ai::decide::decide(
            &self.sidecar.url,
            self.sidecar.key.as_deref(),
            self.sidecar.timeout,
            &request,
        );
        let mut record = ClassifyRecord {
            consumer: "approve".to_owned(),
            message: message_id(call.display),
            answer: None,
            confidence: None,
            model: None,
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            fired: false,
            error: None,
            extra: [("verdictClass".to_owned(), serde_json::json!(class))].into(),
        };
        let safe = match outcome {
            Ok(response) => {
                succeed(&self.breaker);
                record.model = response.routing.map(|routing| routing.model);
                response.answers.get(SAFE).and_then(|answer| answer.noul)
            }
            Err(error) => {
                fail(&self.breaker);
                record.error = Some(self.sidecar.named(&error));
                None
            }
        };
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

pub fn probe(url: &str, checkpoint: &str) -> Result<(), String> {
    let criteria = [("yes", "this is a probe"), ("no", "this is not a probe")]
        .into_iter()
        .map(|(label, text)| (label.to_owned(), text.to_owned()))
        .collect();
    let request = DecisionRequest {
        state: [("message".to_owned(), serde_json::json!("yi setup probe"))].into(),
        questions: [(
            "probe".to_owned(),
            Question::Choice {
                instructions: "Is this a probe?".to_owned(),
                criteria,
            },
        )]
        .into(),
        model: Some(checkpoint.to_owned()),
    };
    let key = yi_ai::auth::api_key("laya");
    let key = key.as_ref().map(|secret| secret.expose());
    let answer = yi_ai::decide::decide(url, key, Duration::from_secs(10), &request)?;
    if answer.answers.contains_key("probe") {
        Ok(())
    } else {
        Err("the answer has no decision".to_owned())
    }
}

pub struct Endpoint {
    pub checkpoint: String,
    pub url: String,
}

pub fn endpoint(config: &UserConfig) -> Option<Endpoint> {
    Some(Endpoint {
        checkpoint: config.models.as_ref()?.classifier.clone()?,
        url: config
            .classifier
            .as_ref()
            .and_then(|block| block.url.clone())
            .unwrap_or_else(|| DEFAULT_URL.to_owned()),
    })
}

pub fn timing(block: &ClassifierConfig) -> Option<Timing> {
    let mode = match (block.approval, block.approve) {
        (Some(mode), _) => mode,
        (None, Some(false)) => ApprovalMode::WaitForUser,
        (None, _) => ApprovalMode::Instant,
    };
    match mode {
        ApprovalMode::Instant => Some(Timing::Instant),
        ApprovalMode::AfterDelay => Some(Timing::AfterDelay(Duration::from_secs(
            block.ask_timeout_secs.unwrap_or(DEFAULT_DELAY_SECS),
        ))),
        ApprovalMode::WaitForUser => None,
    }
}

pub fn attach(session: &AgentSession, cwd: &Path, home: &Path, config: &UserConfig) -> Vec<String> {
    let Some(Endpoint {
        checkpoint: model,
        url,
    }) = endpoint(config)
    else {
        return Vec::new();
    };
    let block = config.classifier.clone().unwrap_or_default();
    let number = |value: &Option<serde_json::Number>, default: f64| {
        value
            .as_ref()
            .and_then(serde_json::Number::as_f64)
            .unwrap_or(default)
    };
    let thresholds = Thresholds {
        allow_at: number(&block.allow_at, DEFAULT_ALLOW_AT),
        allow_destructive_at: number(&block.allow_destructive_at, DEFAULT_ALLOW_DESTRUCTIVE_AT),
        ask_at: number(&block.ask_at, DEFAULT_ASK_AT),
    };
    let timing = timing(&block);
    let chosen = block.approval.is_some() || block.approve.is_some();
    let sidecar = Sidecar {
        url,
        key: yi_ai::auth::api_key("laya").map(|secret| secret.expose().to_owned()),
        model,
        timeout: Duration::from_millis(block.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
        threshold: block
            .threshold
            .as_ref()
            .and_then(serde_json::Number::as_f64),
    };
    let mut warnings = Vec::new();
    if let Some(timing) = timing {
        match (&sidecar.key, session.permission_broker()) {
            (None, _) if chosen => warnings.push(
                "classifier approval is on but no laya key exists yet (`yi serve` makes one, or export LAYA_API_KEY), so the classifier approves nothing"
                    .to_owned(),
            ),
            (Some(_), Some(broker)) => broker.set_approver(Arc::new(Approver::new(
                sidecar.clone(),
                thresholds,
                timing,
                journal(session),
            ))),
            _ => {}
        }
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
