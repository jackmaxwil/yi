use std::sync::Mutex;

use tokio::sync::mpsc::Receiver;
use yi_ai::anthropic::{self, AnthropicOptions, Thinking};
use yi_ai::catalog::Catalog;
use yi_ai::faux::FauxProvider;
use yi_ai::openai::{self, OpenAiOptions};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::run::StreamFn;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::AgentMessage;
use yi_types::model::{LlmContext, Model};

pub fn resolve_model(provider: &str, id: &str) -> Option<Model> {
    Catalog::bundled().get(provider, id).cloned()
}

pub fn available_models() -> Vec<Model> {
    Catalog::bundled().models()
}

fn adaptive(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get("forceAdaptiveThinking"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn anthropic_thinking(model: &Model, level: Option<&str>) -> Thinking {
    match level {
        None | Some("off") => Thinking::Off,
        Some(level) if adaptive(model) => Thinking::Adaptive {
            effort: Some(level.to_owned()),
        },
        Some(level) => Thinking::Budget {
            tokens: match level {
                "minimal" | "low" => 1024,
                "medium" => 4096,
                _ => 16384,
            },
        },
    }
}

pub struct ProviderStream {
    pub api_key: Option<yi_ai::auth::Secret>,
    pub thinking_level: Mutex<Option<String>>,
    pub session_id: Option<String>,
    pub faux: Mutex<FauxProvider>,
}

impl ProviderStream {
    pub fn new(api_key: Option<yi_ai::auth::Secret>, session_id: Option<String>) -> Self {
        Self {
            api_key,
            thinking_level: Mutex::new(None),
            session_id,
            faux: Mutex::new(FauxProvider::default()),
        }
    }

    pub fn set_thinking_level(&self, level: Option<String>) {
        if let Ok(mut current) = self.thinking_level.lock() {
            *current = level;
        }
    }

    pub fn queue_faux(&self, responses: Vec<AgentMessage>) {
        if let Ok(mut faux) = self.faux.lock() {
            faux.append_responses(responses);
        }
    }

    fn key(&self) -> &str {
        self.api_key
            .as_ref()
            .map(yi_ai::auth::Secret::expose)
            .unwrap_or("")
    }
}

impl StreamFn for ProviderStream {
    fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let level = self
            .thinking_level
            .lock()
            .ok()
            .and_then(|level| level.clone());
        match model.api.as_str() {
            "anthropic-messages" => {
                let options = AnthropicOptions {
                    thinking: anthropic_thinking(model, level.as_deref()),
                    cache: true,
                    ..AnthropicOptions::default()
                };
                anthropic::stream(model, context, &options, self.key())
            }
            "openai-completions" => {
                let options = OpenAiOptions {
                    reasoning_effort: level.filter(|value| value != "off"),
                    session_id: self.session_id.clone(),
                    ..OpenAiOptions::default()
                };
                openai::stream(model, context, &options, self.key())
            }
            _ => {
                let (sender, receiver) = tokio::sync::mpsc::channel(64);
                let events = self
                    .faux
                    .lock()
                    .map(|mut faux| faux.stream())
                    .unwrap_or_default();
                for event in events {
                    let _ = sender.try_send(event);
                }
                receiver
            }
        }
    }
}
