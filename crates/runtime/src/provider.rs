use std::sync::Mutex;

use tokio::sync::mpsc::Receiver;
use yi_ai::anthropic::{self, AnthropicOptions, Thinking};
use yi_ai::catalog::Catalog;
use yi_ai::faux::FauxProvider;
use yi_ai::openai::{self, OpenAiOptions};
use yi_ai::openai_responses;
use yi_loop::interrupt::InterruptSignal;
use yi_loop::run::StreamFn;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::AgentMessage;
use yi_types::model::{Effort, LlmContext, Model};

pub fn resolve_model(provider: &str, id: &str) -> Option<Model> {
    Catalog::shared().get(provider, id).cloned()
}

pub fn available_models() -> Vec<Model> {
    Catalog::shared().models()
}

pub fn set_catalog_cache_dir(dir: std::path::PathBuf) {
    Catalog::set_cache_dir(dir);
}

pub fn catalog_cache_dir() -> Option<&'static std::path::Path> {
    Catalog::cache_dir()
}

pub use yi_ai::catalog::PROVIDERS as CATALOG_PROVIDERS;
pub use yi_ai::refresh::{
    DEFAULT_REFRESH_HOURS, MODELS_DEV, age as catalog_age, is_stale as catalog_is_stale,
    list_url as catalog_list_url, refresh as refresh_catalog,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderApi {
    AnthropicMessages,
    OpenAiCompletions,
    OpenAiResponses,
    Faux,
}

fn provider_api(api: &str) -> ProviderApi {
    match api {
        "anthropic-messages" => ProviderApi::AnthropicMessages,
        "openai-completions" => ProviderApi::OpenAiCompletions,
        "openai-responses" => ProviderApi::OpenAiResponses,
        _ => ProviderApi::Faux,
    }
}

fn adaptive(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get("forceAdaptiveThinking"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn anthropic_thinking(model: &Model, effort: Effort) -> Thinking {
    match effort {
        Effort::Off => Thinking::Off,
        effort if adaptive(model) => Thinking::Adaptive {
            effort: Some(effort.to_string()),
        },
        Effort::Minimal | Effort::Low => Thinking::Budget { tokens: 1024 },
        Effort::Medium => Thinking::Budget { tokens: 4096 },
        Effort::High | Effort::XHigh | Effort::Max => Thinking::Budget { tokens: 16384 },
    }
}

pub struct ProviderStream {
    pub api_key: Option<yi_ai::auth::Secret>,
    pub session_id: Option<String>,
    pub faux: Mutex<FauxProvider>,
    long_cache: bool,
    proxy: Option<yi_ai::request::ProxyConfig>,
}

impl ProviderStream {
    pub fn new(api_key: Option<yi_ai::auth::Secret>, session_id: Option<String>) -> Self {
        Self {
            api_key,
            session_id,
            faux: Mutex::new(FauxProvider::default()),
            long_cache: false,
            proxy: None,
        }
    }

    /// An interactive session keeps its stable prefix for an hour; a headless
    /// run keeps the default five minutes.
    #[must_use]
    pub fn with_long_cache(mut self, long_cache: bool) -> Self {
        self.long_cache = long_cache;
        self
    }

    #[must_use]
    pub fn with_proxy(mut self, proxy: Option<yi_ai::request::ProxyConfig>) -> Self {
        self.proxy = proxy;
        self
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
        effort: Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        match provider_api(&model.api) {
            ProviderApi::AnthropicMessages => {
                let options = AnthropicOptions {
                    thinking: anthropic_thinking(model, effort),
                    cache: true,
                    cache_1h: self.long_cache,
                    proxy: self.proxy.clone(),
                    ..AnthropicOptions::default()
                };
                anthropic::stream(model, context, &options, self.key())
            }
            ProviderApi::OpenAiCompletions => {
                let options = OpenAiOptions {
                    reasoning_effort: (effort != Effort::Off).then_some(effort),
                    session_id: self.session_id.clone(),
                    proxy: self.proxy.clone(),
                    ..OpenAiOptions::default()
                };
                openai::stream(model, context, &options, self.key())
            }
            ProviderApi::OpenAiResponses => {
                let options = OpenAiOptions {
                    reasoning_effort: (effort != Effort::Off).then_some(effort),
                    session_id: self.session_id.clone(),
                    proxy: self.proxy.clone(),
                    ..OpenAiOptions::default()
                };
                openai_responses::stream(model, context, &options, self.key())
            }
            ProviderApi::Faux => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luna_routes_to_responses_not_faux() -> Result<(), Box<dyn std::error::Error>> {
        let model = resolve_model("openai", "gpt-5.6-luna")
            .ok_or("bundled catalog missing gpt-5.6-luna")?;
        assert_eq!(provider_api(&model.api), ProviderApi::OpenAiResponses);
        Ok(())
    }
}
