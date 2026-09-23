use std::sync::Arc;

use serde_json::{Map, Value};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::url::{Scheme, Url};

use super::{Page, Resolver};

struct UrlRead {
    inner: Arc<dyn Tool>,
    resolver: Arc<Resolver>,
}

fn page(input: &Map<String, Value>) -> Result<Option<Page>, String> {
    let number = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    let (offset, limit) = (number("offset"), number("limit"));
    if limit == Some(0) {
        return Err("read \"limit\" must be at least 1".to_owned());
    }
    Ok((offset.is_some() || limit.is_some()).then(|| Page {
        offset: offset.unwrap_or(1).saturating_sub(1),
        limit: limit.unwrap_or(usize::MAX),
    }))
}

fn unit(url: &Url) -> &'static str {
    match url.scheme() {
        Scheme::Local => "bytes",
        Scheme::History => "entries",
        _ => "chars",
    }
}

fn address(input: &Map<String, Value>) -> Option<&str> {
    let path = input.get("path")?.as_str()?;
    let (scheme, _) = path.split_once("://")?;
    (!scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_lowercase())).then_some(path)
}

impl Tool for UrlRead {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn schema(&self) -> Value {
        self.inner.schema()
    }

    fn kind(&self) -> ToolKind {
        self.inner.kind()
    }

    fn kind_for(&self, input: &Map<String, Value>) -> ToolKind {
        self.inner.kind_for(input)
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        self.inner.validate(input)
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let Some(raw) = address(&input) else {
            return self.inner.execute(input, context);
        };
        let fetched = raw
            .parse::<Url>()
            .map_err(|error| format!("{raw}: {error}"))
            .and_then(|url| {
                let page = page(&input)?;
                self.resolver
                    .fetch_page(&url, page)
                    .map_err(|error| error.to_string())
            });
        match fetched {
            Ok(fetched) => match fetched.next_offset {
                Some(next) => text_output(format!(
                    "{}\n[more from offset {}; offset and limit count {} here]",
                    fetched.text,
                    next.saturating_add(1),
                    unit(&fetched.url)
                )),
                None => text_output(fetched.text),
            },
            Err(message) => error_output(message),
        }
    }
}

pub fn route_urls(tools: &mut [Arc<dyn Tool>], resolver: &Arc<Resolver>) {
    for tool in tools.iter_mut().filter(|tool| tool.name() == "read") {
        let inner = Arc::clone(tool);
        *tool = Arc::new(UrlRead {
            inner,
            resolver: Arc::clone(resolver),
        });
    }
}
