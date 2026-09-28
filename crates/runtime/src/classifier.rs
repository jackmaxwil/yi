use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use yi_types::classifier::{
    CLASSIFY_ENTRY, ClassifyRecord, DecisionRequest, DecisionResponse, Question,
};
use yi_types::config::UserConfig;
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
pub type Deliver = Arc<dyn Fn(AgentMessage) + Send + Sync>;

pub struct Sidecar {
    pub url: String,
    pub key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub threshold: Option<f64>,
}

#[derive(Default)]
struct Breaker {
    failures: u32,
    paused_until: Option<Instant>,
}

pub struct SkillClassifier {
    sidecar: Sidecar,
    candidates: BTreeMap<String, String>,
    breaker: Mutex<Breaker>,
    record: Record,
    deliver: Deliver,
    pointed: Mutex<std::collections::BTreeSet<String>>,
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
            pointed: Mutex::new(std::collections::BTreeSet::new()),
        })
    }

    pub fn has_pointed(&self, name: &str) -> bool {
        self.pointed
            .lock()
            .is_ok_and(|pointed| pointed.contains(name))
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
        if self.candidates.is_empty() || self.paused() {
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
                if self.fail() {
                    (self.deliver)(self.notice(&error));
                }
                record.error = Some(error);
                (self.record)(record);
                return;
            }
        };
        self.succeed();
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
            && self
                .pointed
                .lock()
                .is_ok_and(|mut pointed| pointed.insert(name.to_owned()))
        {
            (self.deliver)(crate::rules::reminder(format!(
                "Relevant: skill://{name} (the classifier, {confidence:.2})"
            )));
            record.fired = true;
        }
        (self.record)(record);
    }

    fn paused(&self) -> bool {
        let Ok(mut breaker) = self.breaker.lock() else {
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

    fn fail(&self) -> bool {
        let Ok(mut breaker) = self.breaker.lock() else {
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

    fn succeed(&self) {
        if let Ok(mut breaker) = self.breaker.lock() {
            breaker.failures = 0;
        }
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

pub fn attach(
    session: &AgentSession,
    cwd: &Path,
    home: &Path,
    config: &UserConfig,
) -> Result<(), String> {
    let Some(model) = config
        .models
        .as_ref()
        .and_then(|roles| roles.classifier.clone())
    else {
        return Ok(());
    };
    let Some(rules) = session.rules_engine() else {
        return Ok(());
    };
    let block = config.classifier.clone().unwrap_or_default();
    let sidecar = Sidecar {
        url: block.url.unwrap_or_else(|| DEFAULT_URL.to_owned()),
        key: yi_ai::auth::api_key("laya").map(|secret| secret.expose().to_owned()),
        model,
        timeout: Duration::from_millis(block.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
        threshold: block
            .threshold
            .as_ref()
            .and_then(serde_json::Number::as_f64),
    };
    let skills = crate::skills::discover(cwd, home)
        .into_iter()
        .filter(|skill| skill.frontmatter.contains_key("trigger"))
        .map(|skill| (skill.name, skill.description))
        .collect();
    let store = session.store_handle();
    let record: Record = Arc::new(move |record| {
        let (Some(store), Ok(data)) = (store(), serde_json::to_value(&record)) else {
            return;
        };
        let _journaled =
            yi_session::lock_session(&store).append_custom("main", CLASSIFY_ENTRY, Some(data));
    });
    let queue = session.deliver_hook();
    let deliver: Deliver = Arc::new(move |message| queue(message, false));
    rules.set_classifier(Arc::new(SkillClassifier::new(
        sidecar, skills, record, deliver,
    )?));
    Ok(())
}
