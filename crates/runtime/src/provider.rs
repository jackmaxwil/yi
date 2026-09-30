use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::Receiver;
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::auth::{AuthKind, Resolved};
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

/// The entries the catalog files under `dir` hold that do not load, one line each.
pub fn catalog_rejected(dir: &std::path::Path) -> Vec<String> {
    Catalog::bundled().with_cache(dir).rejected().to_vec()
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
}

fn provider_api(api: &str) -> Option<ProviderApi> {
    match api {
        "anthropic-messages" => Some(ProviderApi::AnthropicMessages),
        "openai-completions" => Some(ProviderApi::OpenAiCompletions),
        "openai-responses" => Some(ProviderApi::OpenAiResponses),
        _ => None,
    }
}

pub struct ProviderStream {
    credentials: Arc<Mutex<BTreeMap<String, Arc<Resolved>>>>,
    /// The family's affinity key: the root's id, cut to 64 characters (D314).
    pub session_id: Option<String>,
    pub faux: Arc<Mutex<FauxProvider>>,
    pub(crate) faux_pace: Arc<Mutex<Option<std::time::Duration>>>,
    long_cache: bool,
    /// What the session ledger says of the next gap and write, for its loop requests' TTL
    /// (D315); `None` for a stream no ledger feeds, a child's included.
    ttl_estimate: Mutex<Option<crate::cache_miss::TtlEstimate>>,
    proxy: Option<yi_ai::request::ProxyConfig>,
    routing: Option<serde_json::Value>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    /// `--faux`: every model, whatever its provider, streams from the script (#943).
    force_faux: bool,
}

const HALF_MINUTE: std::time::Duration = std::time::Duration::from_secs(30);

#[expect(
    clippy::disallowed_methods,
    reason = "deciding whether the streaming credential has expired is this fn's job"
)]
fn auth_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

impl ProviderStream {
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            credentials: Arc::default(),
            session_id: session_id.map(|id| id.chars().take(64).collect()),
            faux: Arc::default(),
            faux_pace: Arc::default(),
            long_cache: false,
            ttl_estimate: Mutex::new(None),
            proxy: None,
            routing: None,
            telemetry: None,
            force_faux: false,
        }
    }

    /// A child's stream: the family's credentials, faux script and key, 5-minute marks (D314).
    #[must_use]
    pub(crate) fn for_child(&self) -> Self {
        Self {
            credentials: Arc::clone(&self.credentials),
            session_id: self.session_id.clone(),
            faux: Arc::clone(&self.faux),
            faux_pace: Arc::clone(&self.faux_pace),
            long_cache: false,
            ttl_estimate: Mutex::new(None),
            proxy: self.proxy.clone(),
            routing: self.routing.clone(),
            telemetry: self.telemetry.clone(),
            force_faux: self.force_faux,
        }
    }

    /// Seeds `provider`'s entry with a credential already resolved.
    #[must_use]
    pub fn with_auth(self, provider: &str, resolved: Resolved) -> Self {
        if let Ok(mut credentials) = self.credentials.lock() {
            credentials.insert(provider.to_owned(), Arc::new(resolved));
        }
        self
    }

    /// Per provider on first use, re-resolved outside the map lock 30 s before expiry (D191);
    /// a miss is not kept, so a `yi login` mid-session is seen on the next call (D294).
    pub fn credential(&self, provider: &str) -> Result<Arc<Resolved>, String> {
        let held = self
            .credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.get(provider).cloned());
        if let Some(held) = &held
            && held.expires.is_none_or(|at| auth_now() + HALF_MINUTE < at)
        {
            return Ok(Arc::clone(held));
        }
        match yi_ai::auth::resolve_with_proxy(provider, self.proxy.as_ref()) {
            Some(fresh) => {
                let fresh = Arc::new(fresh);
                if let Ok(mut credentials) = self.credentials.lock() {
                    credentials.insert(provider.to_owned(), Arc::clone(&fresh));
                }
                Ok(fresh)
            }
            // The credential is gone (logout, env unset): keep what was held.
            None => held.ok_or_else(|| yi_ai::auth::missing_message(provider)),
        }
    }

    pub fn has_credential(&self, provider: &str) -> bool {
        self.force_faux
            || provider == yi_ai::faux::FAUX_PROVIDER
            || self.credential(provider).is_ok()
    }

    /// Every request, the family's and a switched model's included, reads the faux script.
    #[must_use]
    pub fn force_faux(mut self, force: bool) -> Self {
        self.force_faux = force;
        self
    }

    pub fn forces_faux(&self) -> bool {
        self.force_faux
    }

    /// An interactive session keeps its stable prefix for an hour; a headless
    /// run keeps the default five minutes.
    #[must_use]
    pub fn with_long_cache(mut self, long_cache: bool) -> Self {
        self.long_cache = long_cache;
        self
    }

    /// The session ledger's latest estimate (`cache_miss::attach`).
    pub fn set_ttl_estimate(&self, estimate: Option<crate::cache_miss::TtlEstimate>) {
        if let Ok(mut held) = self.ttl_estimate.lock() {
            *held = estimate;
        }
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
}

/// The profile's headers plus the account id a ChatGPT login's id_token carried
/// (H4) and the originator the `openai-codex` backend keys on.
fn openai_extra(model: &Model, credential: &Resolved) -> Vec<(String, String)> {
    let mut extra = credential.headers.clone();
    if model.provider == "openai-codex" {
        extra.push(("originator".to_owned(), "yi".to_owned()));
    }
    if let Some(org) = &credential.org {
        extra.push(("chatgpt-account-id".to_owned(), org.clone()));
    }
    extra
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

    fn cache_ttl(&self, model: &Model) -> yi_types::model::Ttl {
        let estimate = self.ttl_estimate.lock().ok().and_then(|held| *held);
        estimate.map_or(yi_types::model::Ttl::Min5, |estimate| {
            estimate.cheapest(model, yi_session::now_ms())
        })
    }
}

/// Per-turn output cap on the OpenAI-style paths: a model's catalog ceiling (131k on flash) let
/// one reasoning turn run to the limit with no tool call and end the trial.
pub const OUTPUT_CEILING: u64 = 32_768;

pub fn output_cap(model: &Model) -> u64 {
    model.max_tokens.min(OUTPUT_CEILING)
}

impl ProviderStream {
    /// Invariant: a model streams with its own provider's credential or sends no request (D294).
    fn stream_raw(
        &self,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let Some(api) = provider_api(&model.api).filter(|_| !self.force_faux) else {
            return self.faux_stream();
        };
        let credential = match self.credential(&model.provider) {
            Ok(credential) => credential,
            Err(missing) => return error_stream(model, &missing),
        };
        let (key, oauth) = (
            credential.secret.expose(),
            credential.kind == AuthKind::Oauth,
        );
        let openai = || OpenAiOptions {
            max_tokens: Some(output_cap(model)),
            reasoning_effort: (effort != Effort::Off).then_some(effort),
            session_id: self.session_id.clone(),
            proxy: self.proxy.clone(),
            routing: self.routing.clone(),
            stop: Some(signal.cut_flag()),
            extra_headers: openai_extra(model, &credential),
            oauth,
            ..OpenAiOptions::default()
        };
        match api {
            ProviderApi::AnthropicMessages => {
                let options = AnthropicOptions {
                    thinking: yi_ai::compat::anthropic_thinking(model, effort),
                    cache_1h: self.long_cache,
                    proxy: self.proxy.clone(),
                    stop: Some(signal.cut_flag()),
                    oauth,
                    extra_headers: credential.headers.clone(),
                    ..AnthropicOptions::default()
                };
                anthropic::stream(model, context, &options, key)
            }
            ProviderApi::OpenAiCompletions => openai::stream(model, context, &openai(), key),
            ProviderApi::OpenAiResponses => {
                openai_responses::stream(model, context, &openai(), key)
            }
        }
    }

    fn faux_stream(&self) -> Receiver<AssistantMessageEvent> {
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

fn error_stream(model: &Model, text: &str) -> Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let mut output = yi_ai::request::empty_assistant(model);
    let _ = sender.try_send(yi_ai::request::fail_message(&mut output, text));
    receiver
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
        assert_eq!(provider_api(&model.api), Some(ProviderApi::OpenAiResponses));
        Ok(())
    }

    /// A child of a `--faux` run spawns on the script, never on a login whose refresh dials out.
    #[test]
    fn a_scripted_stream_admits_any_provider_without_a_credential() {
        let provider = "no-such-provider";
        assert!(!ProviderStream::new(None).has_credential(provider));
        assert!(
            ProviderStream::new(None)
                .force_faux(true)
                .has_credential(provider)
        );
    }
}
