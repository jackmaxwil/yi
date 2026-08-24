use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::advisor::{Advice, AdviceKind, AdvisorySeverity};
use yi_types::model::Model;

use super::AdvisorRuntime;

fn text_outcome(text: &str) -> yi_loop::ToolOutcome {
    yi_loop::ToolOutcome {
        result: yi_types::event::ToolResult {
            content: vec![yi_types::message::Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details: Value::Object(Map::new()),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    }
}
use super::signals::Fired;
use crate::provider::ProviderStream;
use crate::session::{AgentSession, SessionConfig};
use yi_types::model::ToolDef;

/// Fixed reviewer framing (§7.2): review the work log, not the mind — code
/// review of an automated run, never surveillance of hidden reasoning.
pub const ADVISOR_SYSTEM_PROMPT: &str = "You are reviewing the work log of an automated coding run against the task. Judge only what was said and done: user messages, tool calls with their declared intents, tool results, and the agent's emitted prose. Use the advise tool at most once per review with one concrete, specific, actionable note; stay silent when the run is on track. Never repeat advice you already gave. For each claim you are asked to audit, cite the log line that backs it or say UNBACKED.";

fn lock_sink(sink: &Mutex<Vec<Advice>>) -> std::sync::MutexGuard<'_, Vec<Advice>> {
    sink.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The advisor-facing `advise` tool (design V5/V6; omp advise-tool schema,
/// adapted to the V6 vocabulary).
struct AdviseTool {
    sink: Arc<Mutex<Vec<Advice>>>,
}

impl yi_loop::AgentTool for AdviseTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "advise".to_owned(),
            description: "Record one concrete piece of advice for the agent you are watching. Terse, specific, actionable.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "note": {"type": "string", "description": "The advice."},
                    "severity": {"type": "string", "enum": ["note", "warn", "hold"]},
                    "kind": {"type": "string", "enum": ["correction", "risk", "scope", "stop"]},
                    "target": {"type": "string", "description": "Entry id or file path the advice is about."}
                },
                "required": ["note"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a yi_loop::interrupt::InterruptSignal,
    ) -> yi_loop::tool::ToolFuture<'a> {
        Box::pin(async move {
            let note = args
                .get("note")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let severity = match args.get("severity").and_then(Value::as_str) {
                Some("warn") => AdvisorySeverity::Warn,
                Some("hold") => AdvisorySeverity::Hold,
                _ => AdvisorySeverity::Note,
            };
            let kind = match args.get("kind").and_then(Value::as_str) {
                Some("risk") => AdviceKind::Risk,
                Some("scope") => AdviceKind::Scope,
                Some("stop") => AdviceKind::Stop,
                _ => AdviceKind::Correction,
            };
            let target = args
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if !note.is_empty() {
                lock_sink(&self.sink).push(Advice {
                    severity,
                    kind,
                    target,
                    text: note,
                });
            }
            // The emission guard is invisible to the advisor model: every
            // call reads as recorded (omp #3520).
            text_outcome("Recorded.")
        })
    }
}

/// V13 pull tool: full text of a digest-named entry.
struct TranscriptTool {
    runtime: Arc<AdvisorRuntime>,
}

impl yi_loop::AgentTool for TranscriptTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "transcript".to_owned(),
            description:
                "Fetch the full text of a user or assistant entry named in the digest by its id."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"entry_id": {"type": "string"}},
                "required": ["entry_id"]
            }),
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a yi_loop::interrupt::InterruptSignal,
    ) -> yi_loop::tool::ToolFuture<'a> {
        Box::pin(async move {
            let entry_id = args
                .get("entry_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let text = self
                .runtime
                .transcript(entry_id)
                .unwrap_or_else(|| format!("no such entry: {entry_id}"));
            text_outcome(&text)
        })
    }
}

/// Design V5 `LlmReviewer`: its own session, one prompt per review, tools
/// `{advise, transcript}` (read/grep/glob join when the digest proves
/// insufficient). Append-only by construction — each review is a fresh
/// prompt over a stable system prefix.
pub struct LlmReviewer {
    provider: Arc<ProviderStream>,
    model: Model,
    attention: Option<String>,
}

impl LlmReviewer {
    pub fn new(provider: Arc<ProviderStream>, model: Model, attention: Option<String>) -> Self {
        Self {
            provider,
            model,
            attention,
        }
    }

    pub async fn review(
        &self,
        runtime: &Arc<AdvisorRuntime>,
        fired: &[Fired],
        digest_chunk: &str,
    ) -> Vec<Advice> {
        let sink: Arc<Mutex<Vec<Advice>>> = Arc::new(Mutex::new(Vec::new()));
        let mut system = ADVISOR_SYSTEM_PROMPT.to_owned();
        if let Some(attention) = &self.attention {
            system.push_str("\n\n");
            system.push_str(attention);
        }
        let mut session = AgentSession::new(
            SessionConfig {
                system_prompt: system,
                model: self.model.clone(),
                thinking_level: None,
                tool_execution: yi_loop::ExecutionMode::Sequential,
            },
            Arc::clone(&self.provider),
        );
        session.set_tools(vec![
            Arc::new(AdviseTool {
                sink: Arc::clone(&sink),
            }),
            Arc::new(TranscriptTool {
                runtime: Arc::clone(runtime),
            }),
        ]);
        let mut prompt = String::from("Work-log digest since the last review:\n");
        prompt.push_str(digest_chunk);
        if !fired.is_empty() {
            prompt.push_str("\n\nDeterministic signals fired:\n");
            for signal in fired {
                prompt.push_str(&format!(
                    "- {}: {}\n",
                    signal.kind.name(),
                    signal.evidence.join("; ")
                ));
            }
        }
        if session.prompt(&prompt).is_err() {
            return Vec::new();
        }
        session.wait_idle().await;
        if let Some(usage) = session.last_usage() {
            let total = u64::try_from(usage.total_tokens).unwrap_or(0);
            runtime.record_spend(yi_session::now_ms(), total);
        }
        std::mem::take(&mut *lock_sink(&sink))
    }
}
