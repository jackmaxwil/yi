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
use yi_types::message::{AgentMessage, Content};
use yi_types::model::{Effort, LlmContext, Model};

/// OpenRouter's model list omits routing variants (`:exacto`, `:nitro`): one takes its base's entry.
pub fn resolve_model(provider: &str, id: &str) -> Option<Model> {
    resolve_in(Catalog::shared(), provider, id)
}

/// A Cursor id is taken on trust only until the account's model list has been fetched: with a
/// list on disk a typo resolves to nothing, as for every other provider.
fn resolve_in(catalog: &Catalog, provider: &str, id: &str) -> Option<Model> {
    catalog.get(provider, id).cloned().or_else(|| {
        if provider == "cursor" && !catalog.has_provider("cursor") {
            return Some(yi_ai::cursor::model_for(id));
        }
        let (base, _variant) = id.rsplit_once(':').filter(|_| provider == "openrouter")?;
        let mut model = catalog.get(provider, base)?.clone();
        model.id = id.to_owned();
        Some(model)
    })
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
    list_url as catalog_list_url, needs_credential as catalog_needs_credential,
    refresh as refresh_catalog,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderApi {
    AnthropicMessages,
    OpenAiCompletions,
    OpenAiResponses,
    CursorAgent,
}

fn provider_api(api: &str) -> Option<ProviderApi> {
    match api {
        "anthropic-messages" => Some(ProviderApi::AnthropicMessages),
        "openai-completions" => Some(ProviderApi::OpenAiCompletions),
        "openai-responses" => Some(ProviderApi::OpenAiResponses),
        "cursor-agent" => Some(ProviderApi::CursorAgent),
        _ => None,
    }
}

pub struct ProviderStream {
    credentials: Arc<Mutex<BTreeMap<String, Arc<Resolved>>>>,
    /// The family's affinity key: the root's id, cut to 64 characters (D314).
    pub session_id: Option<String>,
    pub faux: Arc<Mutex<FauxProvider>>,
    pub(crate) faux_pace: Arc<Mutex<Option<std::time::Duration>>>,
    pub faux_hold: Arc<FauxHold>,
    pub faux_time: Arc<FauxTime>,
    long_cache: bool,
    /// What the session ledger says of the next gap and write, for its loop requests' TTL
    /// (D315); `None` for a stream no ledger feeds, a child's included.
    ttl_estimate: Mutex<Option<crate::cache_miss::TtlEstimate>>,
    proxy: Option<yi_ai::request::ProxyConfig>,
    routing: Option<serde_json::Value>,
    telemetry: Option<Arc<crate::telemetry::Telemetry>>,
    /// Set by `--faux` or a `faux/…` model: every model, whatever its provider, streams from
    /// the script (#943).
    force_faux: bool,
    cursor: CursorStream,
}

type CursorStream =
    fn(&Arming, &Model, &LlmContext, &str, &InterruptSignal) -> Receiver<AssistantMessageEvent>;

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
            faux_hold: Arc::default(),
            faux_time: Arc::default(),
            long_cache: false,
            ttl_estimate: Mutex::new(None),
            proxy: None,
            routing: None,
            telemetry: None,
            force_faux: false,
            cursor: cursor_stream,
        }
    }

    /// Invariant: only a run loop's turns stream through this, so a Cursor frame in a side call
    /// (compaction, title, rewind, the reviewer) never runs the session's tools.
    pub(crate) fn armed(
        &self,
        tools: Vec<Arc<dyn yi_loop::tool::AgentTool>>,
        cwd: Option<std::path::PathBuf>,
    ) -> ArmedStream<'_> {
        let arming = Arming { tools, cwd };
        ArmedStream {
            provider: self,
            arming,
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
            // A drive holds the root's turn; a child's deltas must not count toward it.
            faux_hold: Arc::default(),
            faux_time: Arc::default(),
            long_cache: false,
            ttl_estimate: Mutex::new(None),
            proxy: self.proxy.clone(),
            routing: self.routing.clone(),
            telemetry: self.telemetry.clone(),
            force_faux: self.force_faux,
            cursor: self.cursor,
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
    pub fn with_forced_faux(mut self, force: bool) -> Self {
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

    /// Each reply streams over the time its recording took (see [`yi_session::cassette`]).
    pub fn queue_faux_timed(&self, responses: Vec<(AgentMessage, std::time::Duration)>) {
        if let Ok(mut faux) = self.faux.lock() {
            faux.append_timed(responses);
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
        self.stream_with(&Arming::default(), model, context, effort, signal)
    }

    fn cache_ttl(&self, model: &Model) -> yi_types::model::Ttl {
        let estimate = self.ttl_estimate.lock().ok().and_then(|held| *held);
        estimate.map_or(yi_types::model::Ttl::Min5, |estimate| {
            estimate.cheapest(model, yi_session::now_ms())
        })
    }
}

pub(crate) struct ArmedStream<'a> {
    provider: &'a ProviderStream,
    arming: Arming,
}

impl StreamFn for ArmedStream<'_> {
    fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        self.provider
            .stream_with(&self.arming, model, context, effort, signal)
    }

    fn cache_ttl(&self, model: &Model) -> yi_types::model::Ttl {
        self.provider.cache_ttl(model)
    }
}

/// Per-turn output cap on the OpenAI-style paths: a model's catalog ceiling (131k on flash) let
/// one reasoning turn run to the limit with no tool call and end the trial.
pub const OUTPUT_CEILING: u64 = 32_768;

pub fn output_cap(model: &Model) -> u64 {
    model.max_tokens.min(OUTPUT_CEILING)
}

impl ProviderStream {
    fn stream_with(
        &self,
        tools: &Arming,
        model: &Model,
        context: &LlmContext,
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let receiver = self.stream_raw(tools, model, context, effort, signal);
        match &self.telemetry {
            Some(telemetry) => telemetry.wrap(model, receiver),
            None => receiver,
        }
    }

    /// Invariant: a model streams with its own provider's credential or sends no request (D294).
    fn stream_raw(
        &self,
        tools: &Arming,
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
            ProviderApi::CursorAgent => (self.cursor)(tools, model, context, key, signal),
        }
    }

    fn faux_stream(&self) -> Receiver<AssistantMessageEvent> {
        self.faux_time
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (events, lag) = self
            .faux
            .lock()
            .map(|mut faux| faux.stream_timed())
            .unwrap_or_default();
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        let pace = self.faux_pace.lock().ok().and_then(|pace| *pace);
        let delays = delays(&events, pace, lag);
        let timed = delays.iter().any(|delay| !delay.is_zero());
        let steps = delays.into_iter().zip(events);
        if timed && self.faux_time.frozen() {
            self.faux_time.schedule(sender, steps.collect());
            return receiver;
        }
        if timed || self.faux_hold.armed() {
            let hold = Arc::clone(&self.faux_hold);
            let time = Arc::clone(&self.faux_time);
            drop(tokio::spawn(paced(sender, steps.collect(), hold, time)));
            return receiver;
        }
        for (_, event) in steps {
            if sender.try_send(event.clone()).is_ok() {
                self.faux_time.count(&event);
            }
        }
        receiver
    }
}

#[derive(Default)]
pub(crate) struct Arming {
    tools: Vec<Arc<dyn yi_loop::tool::AgentTool>>,
    cwd: Option<std::path::PathBuf>,
}

fn cursor_stream(
    arming: &Arming,
    model: &Model,
    context: &LlmContext,
    key: &str,
    signal: &InterruptSignal,
) -> Receiver<AssistantMessageEvent> {
    let tools = Arc::new(offered(&arming.tools, context));
    let stop = cursor_stop(signal);
    let tool = cursor_tool(tools, Arc::clone(&stop));
    yi_ai::cursor::stream(model, context, key, tool, stop, arming.cwd.as_deref())
}

/// A request that offers no tool (a forced answer, the last word) leaves Cursor none to run.
fn offered(
    tools: &[Arc<dyn yi_loop::tool::AgentTool>],
    context: &LlmContext,
) -> Vec<Arc<dyn yi_loop::tool::AgentTool>> {
    let names: Vec<&str> = (context.tools.iter().flatten())
        .map(|tool| tool.name.as_str())
        .collect();
    (tools.iter())
        .filter(|tool| names.contains(&tool.definition().name.as_str()))
        .cloned()
        .collect()
}

fn cursor_stop(signal: &InterruptSignal) -> yi_ai::cursor::StopFn {
    let cut = signal.cut_flag();
    let fired = signal.fired_flag();
    Arc::new(move || {
        cut.load(std::sync::atomic::Ordering::SeqCst)
            || fired.load(std::sync::atomic::Ordering::SeqCst)
    })
}

fn cursor_tool(
    tools: Arc<Vec<Arc<dyn yi_loop::tool::AgentTool>>>,
    tool_stop: yi_ai::cursor::StopFn,
) -> yi_ai::cursor::ToolFn {
    Box::new(move |call_id, name, args| {
        let tools = Arc::clone(&tools);
        let stop = Arc::clone(&tool_stop);
        Box::pin(async move {
            if stop() {
                return "aborted".to_owned();
            }
            let Some(found) = tools.iter().find(|tool| tool.definition().name == name) else {
                return format!("unknown tool {name}");
            };
            if let Err(reason) = found.validate(&args) {
                return reason;
            }
            let local = InterruptSignal::default();
            let outcome = found.execute(&call_id, args, &local).await;
            if stop() {
                return "aborted".to_owned();
            }
            let content = &outcome.result.content;
            let mut parts: Vec<String> = (content.iter())
                .filter_map(|block| match block {
                    Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            let dropped = content.len().saturating_sub(parts.len());
            if dropped > 0 {
                parts.push(format!(
                    "[… {dropped} of {} result blocks dropped: the cursor provider receives text only]",
                    content.len()
                ));
            }
            parts.join("\n")
        })
    })
}

fn error_stream(model: &Model, text: &str) -> Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let mut output = yi_ai::request::empty_assistant(model);
    let _ = sender.try_send(yi_ai::request::fail_message(&mut output, text));
    receiver
}

/// A drive script's `hold`: the faux stream stops before the n-th event of one kind (its wire
/// `type`, e.g. `text_delta`) until `release`, so a turn can be looked at mid-stream.
#[derive(Debug, Default)]
pub struct FauxHold {
    at: Mutex<Option<(String, u32)>>,
    reached: std::sync::atomic::AtomicBool,
    open: tokio::sync::Notify,
}

impl FauxHold {
    pub fn arm(&self, kind: &str, nth: u32) {
        self.reached
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut at) = self.at.lock() {
            *at = Some((kind.to_owned(), nth));
        }
    }

    /// Lets the stream go on; false when it never reached the hold, which a script must hear.
    pub fn release(&self) -> bool {
        let reached = self.at.lock().is_ok_and(|mut at| {
            *at = None;
            self.reached.load(std::sync::atomic::Ordering::SeqCst)
        });
        self.open.notify_waiters();
        reached
    }

    pub fn reached(&self) -> bool {
        self.reached.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn armed(&self) -> bool {
        self.at.lock().is_ok_and(|at| at.is_some())
    }

    async fn pass(&self, event: &yi_types::event::AssistantMessageEvent) {
        if !self.armed() {
            return;
        }
        let kind = serde_json::to_value(event).ok();
        let kind = kind
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(|v| v.as_str());
        // Invariant: the wakeup is enabled before the gate is read, and `reached` is set under the
        // gate's lock, so a `release` racing this event either opens it or sees it reached.
        let opened = self.open.notified();
        tokio::pin!(opened);
        opened.as_mut().enable();
        let stop = self.at.lock().is_ok_and(|mut at| match at.as_mut() {
            Some((want, left)) if Some(want.as_str()) == kind => {
                *left = left.saturating_sub(1);
                let stop = *left == 0;
                self.reached
                    .store(stop, std::sync::atomic::Ordering::SeqCst);
                stop
            }
            _ => false,
        });
        if stop {
            opened.await;
        }
    }
}

/// The pause before each event: a fixed `pace` per streamed word, else the recorded `lag` split
/// evenly over the deltas, so the last delta lands when the recorded model finished.
fn delays(
    events: &[AssistantMessageEvent],
    pace: Option<std::time::Duration>,
    lag: std::time::Duration,
) -> Vec<std::time::Duration> {
    use AssistantMessageEvent as Event;
    let words =
        |event: &Event| matches!(event, Event::TextDelta { .. } | Event::ThinkingDelta { .. });
    let delta = |event: &Event| words(event) || matches!(event, Event::ToolCallDelta { .. });
    let count =
        u32::try_from(events.iter().filter(|event| delta(event)).count()).unwrap_or(u32::MAX);
    let each = lag.checked_div(count).unwrap_or_default();
    events
        .iter()
        .map(|event| match pace {
            Some(pace) if words(event) => pace,
            None if delta(event) => each,
            _ => std::time::Duration::ZERO,
        })
        .collect()
}

async fn paced(
    sender: tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    steps: Vec<Step>,
    hold: Arc<FauxHold>,
    time: Arc<FauxTime>,
) {
    for (delay, event) in steps {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        hold.pass(&event).await;
        if sender.send(event.clone()).await.is_ok() {
            time.count(&event);
        } else {
            return;
        }
    }
}

/// Virtual time for paced faux streams. A drive that freezes its clock freezes this one too: an
/// event waits for [`FauxTime::advance`] to reach its due time instead of for the wall.
#[derive(Debug, Default)]
pub struct FauxTime {
    frozen: Mutex<Option<Frozen>>,
    updates: std::sync::atomic::AtomicU64,
    requests: std::sync::atomic::AtomicU64,
}

type Step = (std::time::Duration, AssistantMessageEvent);
type Pending = (
    tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    std::collections::VecDeque<Step>,
);

#[derive(Debug, Default)]
struct Frozen {
    now: std::time::Duration,
    streams: Vec<Pending>,
}

impl FauxTime {
    pub fn freeze(&self) {
        if let Ok(mut frozen) = self.frozen.lock() {
            frozen.get_or_insert_with(Frozen::default);
        }
    }

    fn frozen(&self) -> bool {
        self.frozen.lock().is_ok_and(|frozen| frozen.is_some())
    }

    /// Stream events sent so far that a screen counts as updates or an end, so a drive can wait
    /// until it has received as many.
    pub fn updates(&self) -> u64 {
        self.updates.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Model requests served so far, which a drive matches to the turns the loop announced.
    pub fn requests(&self) -> u64 {
        self.requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn count(&self, event: &AssistantMessageEvent) {
        if !matches!(event, AssistantMessageEvent::Start { .. }) {
            self.updates
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn schedule(&self, sender: tokio::sync::mpsc::Sender<AssistantMessageEvent>, steps: Vec<Step>) {
        let Ok(mut guard) = self.frozen.lock() else {
            return;
        };
        let Some(frozen) = guard.as_mut() else {
            return;
        };
        let mut due = frozen.now;
        let queued = steps
            .into_iter()
            .map(|(delay, event)| {
                due = due.saturating_add(delay);
                (due, event)
            })
            .collect();
        frozen.streams.push((sender, queued));
        self.release(frozen);
    }

    pub fn advance(&self, by: std::time::Duration) {
        if let Ok(mut guard) = self.frozen.lock()
            && let Some(frozen) = guard.as_mut()
        {
            frozen.now = frozen.now.saturating_add(by);
            self.release(frozen);
        }
    }

    fn release(&self, frozen: &mut Frozen) {
        let now = frozen.now;
        frozen.streams.retain_mut(|(sender, queue)| {
            while queue.front().is_some_and(|(due, _)| *due <= now) {
                let Some((_, event)) = queue.pop_front() else {
                    break;
                };
                if sender.try_send(event.clone()).is_ok() {
                    self.count(&event);
                } else {
                    return false;
                }
            }
            !queue.is_empty()
        });
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
    fn an_openrouter_variant_takes_its_base_entry_and_no_other_provider_does()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = resolve_model("openrouter", "z-ai/glm-5.3-flash").ok_or("no base")?;
        let variant =
            resolve_model("openrouter", "z-ai/glm-5.3-flash:exacto").ok_or("variant refused")?;
        assert_eq!(variant.id, "z-ai/glm-5.3-flash:exacto");
        assert_eq!(
            (variant.cost, variant.context_window),
            (base.cost, base.context_window)
        );
        assert!(resolve_model("openrouter", "z-ai/no-such-model:exacto").is_none());
        assert!(resolve_model("openai", "gpt-5.6-luna:exacto").is_none());
        Ok(())
    }

    #[test]
    fn a_cursor_typo_resolves_only_until_the_account_list_is_fetched()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(resolve_in(&Catalog::bundled(), "cursor", "composer-typo").is_some());
        let dir = std::env::temp_dir().join(format!("yi-cursor-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let list = serde_json::json!({"models": [{"name": "composer-2.5-fast"}]});
        std::fs::write(
            dir.join("cursor.json"),
            yi_ai::refresh::cursor_catalog(&list)?.to_string(),
        )?;
        let fetched = Catalog::bundled().with_cache(&dir);
        std::fs::remove_dir_all(&dir)?;
        assert!(resolve_in(&fetched, "cursor", "composer-2.5-fast").is_some());
        assert!(resolve_in(&fetched, "cursor", "composer-typo").is_none());
        Ok(())
    }

    #[test]
    fn luna_routes_to_responses_not_faux() -> Result<(), Box<dyn std::error::Error>> {
        let model = resolve_model("openai", "gpt-5.6-luna")
            .ok_or("bundled catalog missing gpt-5.6-luna")?;
        assert_eq!(provider_api(&model.api), Some(ProviderApi::OpenAiResponses));
        Ok(())
    }

    #[test]
    fn a_cursor_model_routes_to_cursor_not_faux() -> Result<(), Box<dyn std::error::Error>> {
        let model =
            resolve_model("cursor", "composer-2.5-fast").ok_or("cursor id did not resolve")?;
        assert_eq!(provider_api(&model.api), Some(ProviderApi::CursorAgent));
        Ok(())
    }

    static CURSOR_TOOLS_SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    fn record_tools(
        arming: &Arming,
        model: &Model,
        _context: &LlmContext,
        _key: &str,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut seen) = CURSOR_TOOLS_SEEN.lock() {
            seen.push(arming.tools.len());
        }
        error_stream(model, "recorded")
    }

    #[test]
    fn only_the_armed_stream_lets_a_cursor_turn_run_the_session_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let model = resolve_model("cursor", "composer-2.5-fast").ok_or("cursor id")?;
        let context = LlmContext {
            cache_ttl: yi_types::model::Ttl::Min5,
            system_prompt: String::new(),
            messages: Vec::new(),
            transient: Vec::new(),
            schema: None,
            shared_through: None,
            reuse: yi_types::model::Reuse::OneShot,
            tools: None,
            tool_choice: None,
        };
        let mut provider = ProviderStream::new(None).with_auth(
            "cursor",
            Resolved {
                secret: yi_ai::auth::Secret::new("token".to_owned()),
                kind: AuthKind::Oauth,
                headers: Vec::new(),
                org: None,
                expires: None,
            },
        );
        provider.cursor = record_tools;
        let signal = InterruptSignal::default();
        let tools: Vec<Arc<dyn yi_loop::tool::AgentTool>> = vec![Arc::new(Named("bash"))];
        drop(provider.stream(&model, &context, Effort::Off, &signal));
        drop(
            provider
                .armed(tools, None)
                .stream(&model, &context, Effort::Off, &signal),
        );
        drop(
            provider
                .for_child()
                .stream(&model, &context, Effort::Off, &signal),
        );
        let seen = CURSOR_TOOLS_SEEN.lock().map(|seen| seen.clone());
        assert_eq!(seen.ok(), Some(vec![0, 1, 0]));
        Ok(())
    }

    struct Named(&'static str);

    impl yi_loop::tool::AgentTool for Named {
        fn definition(&self) -> yi_types::model::ToolDef {
            yi_types::model::ToolDef {
                name: self.0.to_owned(),
                description: String::new(),
                parameters: serde_json::json!({}),
                freeform: None,
            }
        }

        fn execute<'a>(
            &'a self,
            _tool_call_id: &'a str,
            _args: serde_json::Map<String, serde_json::Value>,
            _signal: &'a InterruptSignal,
        ) -> yi_loop::tool::ToolFuture<'a> {
            Box::pin(async {
                yi_loop::tool::ToolOutcome {
                    result: yi_loop::tool::error_tool_result(self.0),
                    is_error: false,
                }
            })
        }
    }

    struct Strict(Named);

    impl yi_loop::tool::AgentTool for Strict {
        fn definition(&self) -> yi_types::model::ToolDef {
            self.0.definition()
        }

        fn validate(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<(), String> {
            Err("missing `path`".to_owned())
        }

        fn execute<'a>(
            &'a self,
            tool_call_id: &'a str,
            args: serde_json::Map<String, serde_json::Value>,
            signal: &'a InterruptSignal,
        ) -> yi_loop::tool::ToolFuture<'a> {
            self.0.execute(tool_call_id, args, signal)
        }
    }

    #[test]
    fn a_request_that_offers_no_tool_leaves_cursor_none_to_run() {
        let def = |name: &str| yi_types::model::ToolDef {
            name: name.to_owned(),
            description: String::new(),
            parameters: serde_json::json!({}),
            freeform: None,
        };
        let mut context = LlmContext {
            cache_ttl: yi_types::model::Ttl::Min5,
            system_prompt: String::new(),
            messages: Vec::new(),
            transient: Vec::new(),
            schema: None,
            shared_through: None,
            reuse: yi_types::model::Reuse::OneShot,
            tools: None,
            tool_choice: None,
        };
        let armed: Vec<Arc<dyn yi_loop::tool::AgentTool>> =
            vec![Arc::new(Named("bash")), Arc::new(Named("read"))];
        assert!(offered(&armed, &context).is_empty());
        context.tools = Some(vec![def("read")]);
        let names: Vec<String> = (offered(&armed, &context).iter())
            .map(|tool| tool.definition().name)
            .collect();
        assert_eq!(names, vec!["read".to_owned()]);
    }

    struct Picture;

    impl yi_loop::tool::AgentTool for Picture {
        fn definition(&self) -> yi_types::model::ToolDef {
            Named("look").definition()
        }

        fn execute<'a>(
            &'a self,
            _tool_call_id: &'a str,
            _args: serde_json::Map<String, serde_json::Value>,
            _signal: &'a InterruptSignal,
        ) -> yi_loop::tool::ToolFuture<'a> {
            let mut result = yi_loop::tool::error_tool_result("caption");
            result.content.push(Content::Image {
                data: String::new(),
                mime_type: "image/png".to_owned(),
            });
            Box::pin(async move {
                yi_loop::tool::ToolOutcome {
                    result,
                    is_error: false,
                }
            })
        }
    }

    #[tokio::test]
    async fn an_image_in_a_tool_result_is_named_where_cursor_drops_it() {
        let stop: yi_ai::cursor::StopFn = Arc::new(|| false);
        let tool = cursor_tool(Arc::new(vec![Arc::new(Picture)]), stop);
        assert_eq!(
            tool("c".into(), "look".into(), Default::default()).await,
            "caption\n[… 1 of 2 result blocks dropped: the cursor provider receives text only]"
        );
    }

    #[tokio::test]
    async fn a_cursor_frame_with_invalid_arguments_is_refused_before_the_tool_runs() {
        let stop: yi_ai::cursor::StopFn = Arc::new(|| false);
        let tool = cursor_tool(Arc::new(vec![Arc::new(Strict(Named("read")))]), stop);
        assert_eq!(
            tool("c".into(), "read".into(), Default::default()).await,
            "missing `path`"
        );
    }

    #[tokio::test]
    async fn a_fired_session_stops_the_cursor_stream_and_its_tools() {
        let signal = InterruptSignal::default();
        let stop = cursor_stop(&signal);
        let tool = cursor_tool(Arc::new(vec![Arc::new(Named("read"))]), Arc::clone(&stop));
        assert!(!stop());
        assert_eq!(
            tool("c".into(), "read".into(), Default::default()).await,
            "read"
        );
        signal.fire();
        assert!(stop());
        assert_eq!(
            tool("c".into(), "read".into(), Default::default()).await,
            "aborted"
        );
    }

    /// A child of a `--faux` run spawns on the script, never on a login whose refresh dials out,
    /// and streams from it too.
    #[test]
    fn a_scripted_stream_admits_any_provider_without_a_credential() {
        let provider = "no-such-provider";
        assert!(!ProviderStream::new(None).has_credential(provider));
        let forced = ProviderStream::new(None).with_forced_faux(true);
        assert!(forced.has_credential(provider));
        assert!(forced.for_child().forces_faux());
    }

    fn answer_in(lag_ms: u64) -> (ProviderStream, std::time::Duration) {
        // 48 characters: three text deltas of 16, so a lag of 900 ms is 300 ms a delta.
        let reply = yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_text(&"x".repeat(48))],
            yi_types::message::StopReason::Stop,
        );
        let lag = std::time::Duration::from_millis(lag_ms);
        let provider = ProviderStream::new(None);
        provider.queue_faux_timed(vec![(reply, lag)]);
        (provider, lag)
    }

    fn arrived(receiver: &mut Receiver<AssistantMessageEvent>) -> usize {
        std::iter::from_fn(|| receiver.try_recv().ok()).count()
    }

    /// A frozen clock decides when each delta of a recorded reply lands, and the wall decides
    /// nothing: a scene that ticks 1.9 s must show the same words on every machine.
    #[test]
    fn a_frozen_reply_streams_as_the_clock_advances_and_not_before() {
        let (provider, _) = answer_in(900);
        provider.faux_time.freeze();
        let mut stream = provider.faux_stream();
        // start, text_start: nothing waited yet.
        assert_eq!(arrived(&mut stream), 2);
        provider
            .faux_time
            .advance(std::time::Duration::from_millis(299));
        assert_eq!(arrived(&mut stream), 0);
        provider
            .faux_time
            .advance(std::time::Duration::from_millis(1));
        assert_eq!(arrived(&mut stream), 1);
        assert_eq!(provider.faux_time.updates(), 2);
        provider
            .faux_time
            .advance(std::time::Duration::from_millis(600));
        // delta, delta, text_end, done.
        assert_eq!(arrived(&mut stream), 4);
        assert_eq!(provider.faux_time.updates(), 6);
    }

    /// A stream whose screen has stopped reading counts nothing into the drive's barrier, or
    /// every later `tick` in the scene waits out its five seconds for an event nobody folds.
    #[test]
    fn a_stream_with_no_reader_leaves_the_barrier_count_alone() {
        let (provider, _) = answer_in(900);
        provider.faux_time.freeze();
        let mut stream = provider.faux_stream();
        assert_eq!(arrived(&mut stream), 2);
        let before = provider.faux_time.updates();
        drop(stream);
        provider
            .faux_time
            .advance(std::time::Duration::from_millis(300));
        assert_eq!(provider.faux_time.updates(), before);
    }

    /// The wall path sleeps the recorded lag in total: a reply recorded at 7.7 s that streamed
    /// in 1 s would hide how long a person waits.
    #[tokio::test]
    async fn a_live_reply_takes_the_recorded_time() {
        let (provider, lag) = answer_in(240);
        let started = std::time::Instant::now();
        let mut stream = provider.faux_stream();
        assert_eq!(
            arrived(&mut stream),
            0,
            "the first delta waits for its share"
        );
        let mut count = 0;
        while stream.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 7);
        assert!(started.elapsed() >= lag, "{:?}", started.elapsed());
    }

    /// A cassette with no recorded time is the scripted reply it always was: every event is
    /// there before the first poll.
    #[test]
    fn an_untimed_reply_is_all_there_at_once() {
        let (provider, _) = answer_in(0);
        let mut stream = provider.faux_stream();
        assert_eq!(arrived(&mut stream), 7);
    }
}
