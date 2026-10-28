use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_ai::search::{
    ResultCount, SearchDomain, SearchError, SearchMode, SearchProvider, SearchQuery, SearchRequest,
};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, cut_row, error_output_kind, text_output};
use yi_types::event::ToolErrorKind;

pub(crate) fn wire_search(
    wiring: &crate::wiring::RuntimeWiring,
    tools: &mut Vec<Arc<dyn Tool>>,
    resolver: &Arc<crate::fetch::Resolver>,
) {
    crate::fetch::route_urls(tools, resolver);
    if let Some(provider) = wiring.provider.web_search() {
        tools.push(Arc::new(WebSearchTool::new(provider)));
    }
}

impl crate::provider::ProviderStream {
    pub fn set_web_search(&self, provider: Option<Arc<dyn SearchProvider>>) {
        if let Ok(mut slot) = self.web_search.lock() {
            *slot = provider;
        }
    }

    pub(crate) fn web_search(&self) -> Option<Arc<dyn SearchProvider>> {
        self.web_search.lock().ok().and_then(|slot| slot.clone())
    }
}

pub const EXCERPT_CAP_CHARS: usize = 4_000;
pub const RESULT_CAP_CHARS: usize = 12_000;

pub struct WebSearchTool {
    provider: Arc<dyn SearchProvider>,
}

impl WebSearchTool {
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        Self { provider }
    }

    fn request(input: &Map<String, Value>) -> Result<SearchRequest, String> {
        if let Some(key) = input
            .keys()
            .find(|key| !matches!(key.as_str(), "query" | "count" | "domains" | "mode"))
        {
            return Err(format!("unknown web_search argument: {key}"));
        }
        let query = SearchQuery::new(yi_tools::require_str(input, "query")?)
            .map_err(|error| error.to_string())?;
        let count = match input.get("count") {
            None => ResultCount::default(),
            Some(value) => {
                let count = value
                    .as_u64()
                    .ok_or("count must be an integer between 1 and 25")?;
                let count = u8::try_from(count).map_err(|_| "count must be between 1 and 25")?;
                ResultCount::new(count).map_err(|error| error.to_string())?
            }
        };
        let domains = match input.get("domains") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| "domains must contain only strings".to_owned())
                        .and_then(|domain| {
                            SearchDomain::new(domain).map_err(|error| error.to_string())
                        })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("domains must be an array of domain strings".to_owned()),
        };
        let mode = match input.get("mode") {
            None => SearchMode::default(),
            Some(Value::String(mode)) => mode
                .parse()
                .map_err(|error: SearchError| error.to_string())?,
            Some(_) => return Err("mode must be a string".to_owned()),
        };
        Ok(SearchRequest::new(query, count, domains, mode))
    }

    fn capped(
        text: &str,
        cap: usize,
        kept_label: &str,
        cap_name: &str,
        next: &str,
    ) -> (String, Option<String>) {
        let mut chars = text.chars();
        let kept: String = chars.by_ref().take(cap).collect();
        if chars.next().is_none() {
            return (kept, None);
        }
        let row = cut_row(cap, text.chars().count(), kept_label, cap_name, cap, next);
        (kept, Some(row))
    }

    fn excerpt(text: &str) -> (String, Option<String>) {
        Self::capped(
            text,
            EXCERPT_CAP_CHARS,
            "excerpt chars kept",
            "EXCERPT_CAP_CHARS",
            "use a narrower query or fewer results; no call returns the rest of this excerpt",
        )
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search current public facts through OpenRouter and Exa; the query leaves this machine and a call costs about $0.01; returns cited sources."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search question"},
                "count": {"type": "integer", "minimum": 1, "maximum": 25, "default": 5},
                "domains": {"type": "array", "items": {"type": "string"}},
                "mode": {"type": "string", "enum": ["instant", "fast", "auto", "deep-lite", "deep", "deep-reasoning"], "default": "auto"}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        Self::request(input).map(|_| ())
    }

    fn execute(&self, input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        let request = match Self::request(&input) {
            Ok(request) => request,
            Err(error) => return error_output_kind(error, ToolErrorKind::InvalidArgs),
        };
        let started = std::time::Instant::now();
        let response = match self.provider.search(&request) {
            Ok(response) => response,
            Err(error) => {
                let kind = match error {
                    SearchError::InvalidInput(_) => ToolErrorKind::InvalidArgs,
                    _ => ToolErrorKind::ToolError,
                };
                let mut output = error_output_kind(error.to_string(), kind);
                if let SearchError::Billed { usage, .. } = error {
                    output.result.usage = Some(*usage);
                }
                return output;
            }
        };
        let mut body = response.answer;
        body.push_str("\n\nCitations");
        for (index, citation) in response.citations.iter().enumerate() {
            let (excerpt, cut) = Self::excerpt(&citation.excerpt);
            body.push_str(&format!(
                "\n\n{}. {}\n{}\n{}",
                index.saturating_add(1),
                citation.title,
                citation.url.as_str(),
                excerpt
            ));
            if let Some(cut) = cut {
                body.push('\n');
                body.push_str(&cut);
            }
        }
        let (mut body, cut) = Self::capped(
            &body,
            RESULT_CAP_CHARS,
            "result chars kept",
            "RESULT_CAP_CHARS",
            "use a narrower query or fewer results; no call returns the rest of this result",
        );
        if let Some(cut) = cut {
            body.push('\n');
            body.push_str(&cut);
        }
        let mut output = text_output(crate::fetch::fence_untrusted("web_search", &body));
        output.result.details = json!({
            "provider": response.provider,
            "model": response.model,
            "engine": response.engine.as_str(),
            "mode": response.mode.as_str(),
            "citationCount": response.citations.len(),
            "costUsd": response.usage.cost.total,
            "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "searches": response.searches,
            "upstream": response.upstream,
        });
        output.result.usage = Some(response.usage);
        output
    }
}
