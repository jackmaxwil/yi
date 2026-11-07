use serde_json::Value;
use yi_types::message::AgentMessage;

/// What a request dropped when a 400 named a cache field, for its reply's record.
#[derive(Default)]
pub struct Stripped(std::cell::Cell<Option<&'static str>>);

impl Stripped {
    pub(super) fn set(&self, field: &'static str) {
        self.0.set(Some(field));
    }

    pub fn take(&self) -> Option<&'static str> {
        self.0.take()
    }
}

/// The reply's record of its own request: a dead stream's resend, and the fields a 400 dropped.
pub fn note_reply(output: &mut AgentMessage, resent: Option<String>, stripped: &Stripped) {
    if let Some(first_error) = resent {
        note_resend(output, &first_error);
    }
    record_stripped(output, stripped);
}

/// The cache field a 400 refused, recorded once on the reply that came back without it.
pub fn record_stripped(output: &mut AgentMessage, stripped: &Stripped) {
    if let Some(field) = stripped.take() {
        note_stripped(output, field);
    }
}

fn push_diagnostic(
    output: &mut AgentMessage,
    kind: &str,
    key: &str,
    value: Value,
    error: Option<yi_types::message::DiagnosticErrorInfo>,
) {
    if let AgentMessage::Assistant {
        diagnostics,
        timestamp,
        ..
    } = output
    {
        let notes = diagnostics.get_or_insert_with(Vec::new);
        if kind == "upstream" && notes.iter().any(|note| note.diagnostic_type == kind) {
            return;
        }
        let mut details = serde_json::Map::new();
        details.insert(key.to_owned(), value);
        notes.push(yi_types::message::AssistantMessageDiagnostic {
            diagnostic_type: kind.to_owned(),
            timestamp: *timestamp,
            error,
            details: Some(details),
        });
    }
}

/// The cache field a 400 refused, kept once on the reply that came back without it.
pub fn note_stripped(output: &mut AgentMessage, field: &str) {
    push_diagnostic(output, "cache_stripped", "field", Value::from(field), None);
}

pub fn note_resend(output: &mut AgentMessage, first_error: &str) {
    let error = yi_types::message::DiagnosticErrorInfo {
        name: None,
        message: first_error.to_owned(),
        stack: None,
        code: None,
    };
    push_diagnostic(
        output,
        "stream_resent",
        "resends",
        Value::from(1u32),
        Some(error),
    );
}

/// The upstream an OpenRouter turn ran on, kept once: every chunk names it, and so does the record.
pub fn note_upstream(output: &mut AgentMessage, upstream: &str) {
    push_diagnostic(output, "upstream", "provider", Value::from(upstream), None);
}
