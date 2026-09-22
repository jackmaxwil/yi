use std::sync::Arc;

use serde_json::{Map, Value};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::url::Url;

use super::Resolver;

struct UrlRead {
    inner: Arc<dyn Tool>,
    resolver: Arc<Resolver>,
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
            .and_then(|url| self.resolver.fetch(&url).map_err(|error| error.to_string()));
        match fetched {
            Ok(fetched) => text_output(fetched.text),
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
