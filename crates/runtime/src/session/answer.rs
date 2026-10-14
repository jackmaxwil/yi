use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use yi_loop::{LoopConfig, TurnSnapshot};
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};

use super::{InterceptStopFn, Shared};

const FINISH: &str = "Finish with `answer`: only `summary` keeps the reply above as the answer once the work is fixed; `text` replaces it.";

/// A refused answer: its text, the file `edit` patches it in, and whether it is a plain-text
/// reply already on screen.
#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub(crate) text: String,
    file: Option<PathBuf>,
    shown: bool,
}

impl Draft {
    /// What resubmitting it submits: the file as `edit` left it, or None when the reply on
    /// screen stands.
    pub(crate) fn submitted(&self) -> Option<String> {
        match &self.file {
            Some(file) => Some(std::fs::read_to_string(file).unwrap_or_else(|_| self.text.clone())),
            None => (!self.shown).then(|| self.text.clone()),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) enum Answering {
    #[default]
    Open,
    /// `noted`: a plain-text stop was sent back, so the next one ends the turn.
    Held {
        draft: Draft,
        noted: bool,
    },
    Accepted(String),
    /// The reply on screen passed: the turn ends with nothing appended.
    Stood,
    /// Invariant: time is up, so an `answer` the last word refused is the answer, unjudged.
    LastWord(Option<Draft>),
    /// Invariant: the turn is over, so no host note extends it; only typed input, a task, mail
    /// or child news opens the next one.
    Answered,
}

pub(super) fn state(shared: &Shared) -> MutexGuard<'_, Answering> {
    shared.answer.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn last_word(shared: &Shared) {
    let mut state = state(shared);
    let held = match std::mem::take(&mut *state) {
        Answering::Held { draft, .. } => Some(draft),
        _ => None,
    };
    *state = Answering::LastWord(held);
}

#[derive(Clone)]
pub(crate) struct AnswerHandle(pub(super) Arc<Shared>);

impl AnswerHandle {
    pub(crate) fn held(&self) -> Option<Draft> {
        match &*state(&self.0) {
            Answering::Held { draft, .. } => Some(draft.clone()),
            _ => None,
        }
    }

    pub(crate) fn judge(&self, draft: &str) -> Option<String> {
        let coupling = self.0.coupling.lock().ok()?.clone()?;
        let message = answer_of(&AgentMessage::task("", 0), draft.to_owned());
        let snapshot = TurnSnapshot {
            message: &message,
            tool_results: &[],
        };
        (coupling.intercept_stop)(&snapshot).map(|refusal| refusal.plain_text())
    }

    pub(crate) fn hold(&self, text: String, file: Option<PathBuf>) {
        let mut state = state(&self.0);
        let noted = matches!(&*state, Answering::Held { noted: true, .. });
        let draft = Draft {
            text,
            file,
            shown: false,
        };
        *state = Answering::Held { draft, noted };
    }

    pub(crate) fn accept(&self, draft: String) {
        *state(&self.0) = Answering::Accepted(draft);
    }

    pub(crate) fn stand(&self) {
        *state(&self.0) = Answering::Stood;
    }
}

fn answer_of(turn: &AgentMessage, text: String) -> AgentMessage {
    let (api, provider, model, response_model, timestamp) = match turn {
        AgentMessage::Assistant {
            api,
            provider,
            model,
            response_model,
            timestamp,
            ..
        } => (api, provider, model, response_model.clone(), *timestamp),
        _ => (&String::new(), &String::new(), &String::new(), None, 0),
    };
    AgentMessage::Assistant {
        content: vec![Content::Text {
            text,
            text_signature: None,
        }],
        api: api.clone(),
        provider: provider.clone(),
        model: model.clone(),
        response_model,
        response_id: None,
        diagnostics: None,
        usage: Usage::unknown(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    }
}

/// The `text` of an `answer` call the turn made, `Some(None)` for a call without one.
fn answer_call(message: &AgentMessage) -> Option<Option<String>> {
    let AgentMessage::Assistant { content, .. } = message else {
        return None;
    };
    content.iter().find_map(|block| match block {
        Content::ToolCall {
            name, arguments, ..
        } if name == "answer" => Some(
            (arguments.get("text").and_then(Value::as_str))
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned),
        ),
        _ => None,
    })
}

fn text_stop(snapshot: &TurnSnapshot) -> Option<String> {
    let AgentMessage::Assistant {
        content,
        stop_reason: StopReason::Stop,
        ..
    } = snapshot.message
    else {
        return None;
    };
    let calls = content
        .iter()
        .any(|block| matches!(block, Content::ToolCall { .. }));
    (!calls && snapshot.tool_results.is_empty()).then(|| yi_types::message::join_text(content, ""))
}

/// A refusal of a plain-text stop: the gate's own note, and the reply it refused when the
/// note asks for `answer`; it holds the reply only once the loop delivers the note.
type Verdict = (AgentMessage, Option<String>);

fn judge_stop(shared: &Shared, gate: &InterceptStopFn, snapshot: &TurnSnapshot) -> Option<Verdict> {
    let text = text_stop(snapshot)?;
    let last = matches!(
        &*state(shared),
        Answering::Held { noted: true, .. } | Answering::LastWord(_)
    );
    if last {
        *state(shared) = Answering::Answered;
        return None;
    }
    let Some(refusal) = gate(snapshot) else {
        *state(shared) = Answering::Answered;
        return None;
    };
    Some((refusal, Some(text).filter(|text| !text.trim().is_empty())))
}

fn delivered(shared: &Shared, (mut refusal, reply): Verdict) -> AgentMessage {
    let Some(draft) = reply else {
        return refusal;
    };
    let draft = Draft {
        text: draft,
        file: None,
        shown: true,
    };
    *state(shared) = Answering::Held { draft, noted: true };
    if let AgentMessage::Custom {
        content: UserContent::Text(text),
        ..
    }
    | AgentMessage::User {
        content: UserContent::Text(text),
        ..
    } = &mut refusal
    {
        text.push('\n');
        text.push_str(FINISH);
    }
    refusal
}

pub(super) fn wire(
    config: &mut LoopConfig,
    shared: &Arc<Shared>,
    observe: Option<Arc<super::TurnObserveFn>>,
    gate: Option<Arc<InterceptStopFn>>,
) {
    let appended = Arc::clone(shared);
    config.answer = Some(Box::new(move |snapshot| {
        let mut state = state(&appended);
        let cut = matches!(
            snapshot.message,
            AgentMessage::Assistant {
                stop_reason: StopReason::Length,
                ..
            }
        );
        let text = match (&*state, answer_call(snapshot.message)) {
            (Answering::Accepted(text), _) => Some(text.clone()),
            (Answering::Stood, _) => None,
            (Answering::LastWord(_), Some(Some(text))) if !cut => Some(text),
            (Answering::LastWord(Some(held)), Some(None)) if !cut => held.submitted(),
            _ => return None,
        };
        *state = Answering::Answered;
        Some(text.map(|text| answer_of(snapshot.message, text)))
    }));
    let Some(gate) = gate else {
        return;
    };
    let pending: Arc<Mutex<Option<Verdict>>> = Arc::default();
    let (judged, judging) = (Arc::clone(&pending), Arc::clone(shared));
    config.prepare_next_turn = Some(Box::new(move |snapshot| {
        if let Some(observe) = &observe {
            observe(snapshot);
        }
        let verdict = judge_stop(&judging, gate.as_ref(), snapshot);
        *judged.lock().unwrap_or_else(PoisonError::into_inner) = verdict;
        None
    }));
    let answered = Arc::clone(shared);
    config.intercept_stop = Some(Box::new(move |_| {
        let verdict = pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()?;
        let open = !matches!(&*state(&answered), Answering::Answered);
        open.then(|| delivered(&answered, verdict))
    }));
}
