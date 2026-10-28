use serde_json::{Value, json};
use yi_types::message::Usage;
use yi_types::model::Model;

use crate::auth::Secret;
use crate::request::{PostJsonError, ProxyConfig};

const RESPONSE_CAP: usize = 8 * 1024 * 1024;
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const INSTRUCTION: &str = "Perform exactly one web search for the user's query, then answer in at most 150 words using only what the search returned, citing each claim with a markdown link to its source.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery(String);

impl SearchQuery {
    pub fn new(value: impl Into<String>) -> Result<Self, SearchError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SearchError::InvalidInput(
                "query must not be empty".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultCount(u8);

impl ResultCount {
    pub fn new(value: u8) -> Result<Self, SearchError> {
        if !(1..=25).contains(&value) {
            return Err(SearchError::InvalidInput(
                "count must be between 1 and 25".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl Default for ResultCount {
    fn default() -> Self {
        Self(5)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchDomain(String);

impl SearchDomain {
    pub fn new(value: impl Into<String>) -> Result<Self, SearchError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SearchError::InvalidInput(
                "domains must contain only non-empty strings".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchEngine(String);

impl SearchEngine {
    pub fn new(value: impl Into<String>) -> Result<Self, SearchError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SearchError::InvalidInput(
                "webSearch.engine must not be empty".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self("exa".to_owned())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchMode {
    Instant,
    Fast,
    #[default]
    Auto,
    DeepLite,
    Deep,
    DeepReasoning,
}

impl SearchMode {
    pub const ALL: [Self; 6] = [
        Self::Instant,
        Self::Fast,
        Self::Auto,
        Self::DeepLite,
        Self::Deep,
        Self::DeepReasoning,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Fast => "fast",
            Self::Auto => "auto",
            Self::DeepLite => "deep-lite",
            Self::Deep => "deep",
            Self::DeepReasoning => "deep-reasoning",
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = SearchError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .ok_or_else(|| {
                SearchError::InvalidInput(format!(
                    "mode must be instant, fast, auto, deep-lite, deep or deep-reasoning; got {value}"
                ))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchUrl(String);

impl SearchUrl {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: SearchQuery,
    pub count: ResultCount,
    pub domains: Vec<SearchDomain>,
    pub mode: SearchMode,
}

impl SearchRequest {
    pub fn new(
        query: SearchQuery,
        count: ResultCount,
        domains: Vec<SearchDomain>,
        mode: SearchMode,
    ) -> Self {
        Self {
            query,
            count,
            domains,
            mode,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchCitation {
    pub title: String,
    pub url: SearchUrl,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoute {
    provider: String,
    model: String,
}

impl SearchRoute {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchCount(u64);

impl SearchCount {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct SearchResponse {
    pub answer: String,
    pub citations: Vec<SearchCitation>,
    pub route: SearchRoute,
    pub engine: SearchEngine,
    pub mode: SearchMode,
    pub searches: SearchCount,
    pub upstream: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid web search request: {0}")]
    InvalidInput(String),
    #[error("web search is unavailable: no OpenRouter credential resolved")]
    MissingCredential,
    #[error("web search transport failed: {0}")]
    Transport(String),
    #[error("web search HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("malformed web search response: {0}")]
    Protocol(String),
    #[error("web search returned no citations")]
    NoCitations,
    #[error("{error}")]
    Billed {
        route: SearchRoute,
        error: Box<SearchError>,
        usage: Box<Usage>,
    },
    #[error("{0}")]
    Other(String),
}

pub trait SearchProvider: Send + Sync {
    fn search(&self, request: &SearchRequest) -> Result<SearchResponse, SearchError>;
}

pub struct OpenRouterSearch {
    model: Model,
    credential: Secret,
    engine: SearchEngine,
    proxy: Option<ProxyConfig>,
    routing: Option<Value>,
}

impl OpenRouterSearch {
    pub fn new(
        model: Model,
        credential: Option<Secret>,
        engine: SearchEngine,
        proxy: Option<ProxyConfig>,
        routing: Option<Value>,
    ) -> Result<Self, SearchError> {
        let credential = credential.ok_or(SearchError::MissingCredential)?;
        Ok(Self {
            model,
            credential,
            engine,
            proxy,
            routing,
        })
    }

    fn redact(&self, text: String) -> String {
        text.replace(self.credential.expose(), "***")
    }

    fn route(&self) -> SearchRoute {
        SearchRoute::new("openrouter", self.model.id.clone())
    }

    fn billed(&self, error: SearchError, usage: Usage) -> SearchError {
        SearchError::Billed {
            route: self.route(),
            error: Box::new(error),
            usage: Box::new(usage),
        }
    }

    fn parse(&self, body: Value, request: &SearchRequest) -> Result<SearchResponse, SearchError> {
        let empty_usage = Value::Null;
        let usage = body.get("usage").unwrap_or(&empty_usage);
        let parsed_usage = body
            .get("usage")
            .map(|usage| crate::openai::parse_chunk_usage(usage, &self.model))
            .unwrap_or_else(Usage::unknown);
        let parsed = (|| -> Result<SearchResponse, SearchError> {
            let message = body
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"))
                .ok_or_else(|| SearchError::Protocol("choices[0].message is missing".to_owned()))?;
            let answer = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| SearchError::Protocol("message.content is missing".to_owned()))?
                .to_owned();
            let annotations = message
                .get("annotations")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    SearchError::Protocol("message.annotations is missing".to_owned())
                })?;
            let citations = annotations
                .iter()
                .filter(|annotation| {
                    annotation.get("type").and_then(Value::as_str) == Some("url_citation")
                })
                .map(|annotation| {
                    let source = annotation.get("url_citation").ok_or_else(|| {
                        SearchError::Protocol("url_citation is missing".to_owned())
                    })?;
                    let text = |field: &str| {
                        source.get(field).and_then(Value::as_str).ok_or_else(|| {
                            SearchError::Protocol(format!("url_citation.{field} is missing"))
                        })
                    };
                    Ok(SearchCitation {
                        title: text("title")?.to_owned(),
                        url: SearchUrl::new(text("url")?),
                        excerpt: text("content")?.to_owned(),
                    })
                })
                .collect::<Result<Vec<_>, SearchError>>()?;
            if citations.is_empty() {
                return Err(SearchError::NoCitations);
            }
            let searches = usage
                .get("server_tool_use_details")
                .or_else(|| usage.get("server_tool_use"))
                .and_then(|details| details.get("web_search_requests"))
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    SearchError::Protocol("usage server search count is missing".to_owned())
                })?;
            Ok(SearchResponse {
                answer,
                citations,
                route: self.route(),
                engine: self.engine.clone(),
                mode: request.mode,
                searches: SearchCount::new(searches),
                upstream: body
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                usage: parsed_usage.clone(),
            })
        })();
        parsed.map_err(|error| self.billed(error, parsed_usage))
    }
}

impl SearchProvider for OpenRouterSearch {
    fn search(&self, request: &SearchRequest) -> Result<SearchResponse, SearchError> {
        let mut parameters = json!({
            "engine": self.engine.as_str(),
            "mode": request.mode.as_str(),
            "max_results": request.count.get(),
            "max_total_results": request.count.get(),
            "max_uses": 1,
        });
        if !request.domains.is_empty() {
            parameters["allowed_domains"] = json!(
                request
                    .domains
                    .iter()
                    .map(SearchDomain::as_str)
                    .collect::<Vec<_>>()
            );
        }
        let mut body = json!({
            "model": self.model.id,
            "stream": false,
            "messages": [
                {"role": "system", "content": INSTRUCTION},
                {"role": "user", "content": request.query.as_str()},
            ],
            "tools": [{"type": "openrouter:web_search", "parameters": parameters}],
        });
        let options = crate::openai::OpenAiOptions {
            routing: self.routing.clone(),
            ..Default::default()
        };
        if let Some(routing) = crate::openai::routing_params(&self.model, &options, false) {
            body["provider"] = routing;
        }
        let encoded =
            serde_json::to_string(&body).map_err(|error| SearchError::Other(error.to_string()))?;
        let url = format!(
            "{}/chat/completions",
            self.model.base_url.trim_end_matches('/')
        );
        let headers = [(
            "authorization",
            format!("Bearer {}", self.credential.expose()),
        )];
        let response = crate::request::post_json(
            &url,
            &headers,
            &encoded,
            self.proxy.as_ref(),
            RESPONSE_CAP,
            READ_TIMEOUT,
        )
        .map_err(|error| match error {
            PostJsonError::Transport(message) => SearchError::Transport(self.redact(message)),
            PostJsonError::Http { status, .. } => SearchError::Http {
                status,
                message: match status {
                    400..=499 => {
                        "client error; check webSearch.engine, the model id, and the OpenRouter key"
                    }
                    500..=599 => "server error; try again later",
                    _ => "unexpected HTTP status",
                }
                .to_owned(),
            },
            PostJsonError::Protocol(message) => self.billed(
                SearchError::Protocol(self.redact(message)),
                Usage::unknown(),
            ),
        })?;
        self.parse(response, request)
    }
}
