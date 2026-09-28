use std::sync::{Arc, Mutex};

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
    auth: Mutex<AuthCell>,
    provider: String,
    pub session_id: Option<String>,
    pub faux: Mutex<FauxProvider>,
    pub(crate) faux_pace: Mutex<Option<std::time::Duration>>,
    long_cache: bool,
    proxy: Option<yi_ai::request::ProxyConfig>,
    routing: Option<serde_json::Value>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    oauth: bool,
    org: Option<String>,
    headers: Vec<(String, String)>,
}

const HALF_MINUTE: std::time::Duration = std::time::Duration::from_secs(30);

#[expect(
    clippy::disallowed_methods,
    reason = "deciding whether the streaming credential has expired is this fn's job"
)]
fn auth_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// The credential a stream is using, re-resolved when it expires so a days-long
/// TUI, ACP or daemon session refreshes instead of failing turn by turn (D191).
struct AuthCell {
    secret: Option<yi_ai::auth::Secret>,
    expires: Option<std::time::SystemTime>,
}

impl ProviderStream {
    pub fn new(api_key: Option<yi_ai::auth::Secret>, session_id: Option<String>) -> Self {
        Self {
            auth: Mutex::new(AuthCell {
                secret: api_key,
                expires: None,
            }),
            provider: String::new(),
            session_id,
            faux: Mutex::new(FauxProvider::default()),
            faux_pace: Mutex::new(None),
            long_cache: false,
            proxy: None,
            routing: None,
            telemetry: None,
            oauth: false,
            org: None,
            headers: Vec::new(),
        }
    }

    /// The resolved credential's shape and the login profile's headers (D191):
    /// a stored OAuth token streams as Bearer and carries whatever that file holds.
    #[must_use]
    pub fn with_auth(mut self, provider: &str, resolved: &yi_ai::auth::Resolved) -> Self {
        self.provider = provider.to_owned();
        self.oauth = resolved.kind == yi_ai::auth::AuthKind::Oauth;
        self.org = resolved.org.clone();
        self.headers = resolved.headers.clone();
        if let Ok(mut cell) = self.auth.lock() {
            cell.secret = Some(yi_ai::auth::Secret::new(
                resolved.secret.expose().to_owned(),
            ));
            cell.expires = resolved.expires;
        }
        self
    }

    /// An interactive session keeps its stable prefix for an hour; a headless
    /// run keeps the default five minutes.
    #[must_use]
    pub fn with_long_cache(mut self, long_cache: bool) -> Self {
        self.long_cache = long_cache;
        self
    }

    #[must_use]
    pub fn with_routing(mut self, routing: Option<serde_json::Value>) -> Self {
        self.routing = routing;
        self
    }

    pub fn with_proxy(mut self, proxy: Option<yi_ai::request::ProxyConfig>) -> Self {
        self.proxy = proxy;
        self
    }

    pub fn with_telemetry(mut self, telemetry: Option<Arc<crate::telemetry::Telemetry>>) -> Self {
        self.telemetry = telemetry;
        self
    }

    pub fn queue_faux(&self, responses: Vec<AgentMessage>) {
        if let Ok(mut faux) = self.faux.lock() {
            faux.append_responses(responses);
        }
    }

    /// The profile's headers plus the account id a ChatGPT login's id_token carried
    /// (H4) and the originator the `openai-codex` backend keys on.
    fn openai_extra(&self, model: &Model) -> Vec<(String, String)> {
        let mut extra = self.headers.clone();
        if model.provider == "openai-codex" {
            extra.push(("originator".to_owned(), "yi".to_owned()));
        }
        if let Some(org) = &self.org {
            extra.push(("chatgpt-account-id".to_owned(), org.clone()));
        }
        extra
    }

    fn key(&self) -> String {
        if self.oauth && !self.provider.is_empty() {
            let expired = self
                .auth
                .lock()
                .map(|cell| {
                    cell.expires
                        .is_some_and(|at| auth_now() + HALF_MINUTE >= at)
                })
                .unwrap_or(false);
            // resolve_with_proxy refreshes under the store's cross-process lock.
            if expired
                && let Some(fresh) =
                    yi_ai::auth::resolve_with_proxy(&self.provider, self.proxy.as_ref())
                && let Ok(mut cell) = self.auth.lock()
            {
                cell.secret = Some(fresh.secret);
                cell.expires = fresh.expires;
            }
        }
        self.auth
            .lock()
            .ok()
            .and_then(|cell| cell.secret.as_ref().map(|s| s.expose().to_owned()))
            .unwrap_or_default()
    }
}

impl StreamFn for ProviderStream {
    fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let receiver = self.stream_raw(model, context, effort, signal);
        match &self.telemetry {
            Some(telemetry) => telemetry.wrap(model, receiver),
            None => receiver,
        }
    }
}

/// Per-turn output cap on the OpenAI-style paths: a model's catalog ceiling (131k on flash) let
/// one reasoning turn run to the limit with no tool call and end the trial.
pub const OUTPUT_CEILING: u64 = 32_768;

pub fn output_cap(model: &Model) -> u64 {
    model.max_tokens.min(OUTPUT_CEILING)
}

impl ProviderStream {
    fn stream_raw(
        &self,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        match provider_api(&model.api) {
            ProviderApi::AnthropicMessages => {
                let options = AnthropicOptions {
                    thinking: anthropic_thinking(model, effort),
                    cache_1h: self.long_cache,
                    proxy: self.proxy.clone(),
                    stop: Some(signal.cut_flag()),
                    oauth: self.oauth,
                    extra_headers: self.headers.clone(),
                    ..AnthropicOptions::default()
                };
                anthropic::stream(model, context, &options, &self.key())
            }
            ProviderApi::OpenAiCompletions => {
                let options = OpenAiOptions {
                    max_tokens: Some(output_cap(model)),
                    reasoning_effort: (effort != Effort::Off).then_some(effort),
                    session_id: self.session_id.clone(),
                    proxy: self.proxy.clone(),
                    routing: self.routing.clone(),
                    stop: Some(signal.cut_flag()),
                    extra_headers: self.openai_extra(model),
                    oauth: self.oauth,
                    ..OpenAiOptions::default()
                };
                openai::stream(model, context, &options, &self.key())
            }
            ProviderApi::OpenAiResponses => {
                let options = OpenAiOptions {
                    max_tokens: Some(output_cap(model)),
                    reasoning_effort: (effort != Effort::Off).then_some(effort),
                    session_id: self.session_id.clone(),
                    proxy: self.proxy.clone(),
                    routing: self.routing.clone(),
                    stop: Some(signal.cut_flag()),
                    extra_headers: self.openai_extra(model),
                    oauth: self.oauth,
                    ..OpenAiOptions::default()
                };
                openai_responses::stream(model, context, &options, &self.key())
            }
            ProviderApi::Faux => {
                let events = self
                    .faux
                    .lock()
                    .map(|mut faux| faux.stream())
                    .unwrap_or_default();
                let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
                let pace = self.faux_pace.lock().ok().and_then(|pace| *pace);
                if let Some(pace) = pace {
                    drop(tokio::spawn(paced(sender, events, pace)));
                    return receiver;
                }
                for event in events {
                    let _ = sender.try_send(event);
                }
                receiver
            }
        }
    }
}

async fn paced(
    sender: tokio::sync::mpsc::Sender<yi_types::event::AssistantMessageEvent>,
    events: Vec<yi_types::event::AssistantMessageEvent>,
    pace: std::time::Duration,
) {
    use yi_types::event::AssistantMessageEvent as Event;
    for event in events {
        if matches!(event, Event::TextDelta { .. } | Event::ThinkingDelta { .. }) {
            tokio::time::sleep(pace).await;
        }
        if sender.send(event).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_cap_clamps_the_catalog_never_raises_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut model = resolve_model("openrouter", "z-ai/glm-5.3-flash")
            .ok_or("bundled catalog missing z-ai/glm-5.3-flash")?;
        assert_eq!(model.max_tokens, 131_072);
        assert_eq!(output_cap(&model), OUTPUT_CEILING);
        model.max_tokens = 16_384;
        assert_eq!(output_cap(&model), 16_384);
        Ok(())
    }

    #[test]
    fn luna_routes_to_responses_not_faux() -> Result<(), Box<dyn std::error::Error>> {
        let model = resolve_model("openai", "gpt-5.6-luna")
            .ok_or("bundled catalog missing gpt-5.6-luna")?;
        assert_eq!(provider_api(&model.api), ProviderApi::OpenAiResponses);
        Ok(())
    }
}
