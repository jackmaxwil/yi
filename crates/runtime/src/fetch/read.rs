use std::sync::Arc;

use serde_json::{Map, Value};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::url::{Scheme, Url};

use super::{Page, Resolver};

/// Incident: an unpaged `read history://<id>` of a 5 MB session put 4.86 MB in one tool result.
const LISTING_BYTES: usize = 50 * 1024;

struct UrlRead {
    inner: Arc<dyn Tool>,
    resolver: Arc<Resolver>,
}

fn page(input: &Map<String, Value>) -> Result<Option<Page>, String> {
    let page = Page::from_payload(input, "read")?;
    Ok(page.map(|page| Page {
        offset: page.offset.saturating_sub(1),
        ..page
    }))
}

fn unit(url: &Url) -> &'static str {
    match url.scheme() {
        Scheme::Local => "bytes",
        Scheme::History => "entries",
        _ => "chars",
    }
}

/// Whole entries up to the budget, the first one clipped if it alone is over; the bare listing
/// and `since/` both page by `offset`, so that is the next call.
fn capped(url: &Url, served_by: &str, text: String) -> String {
    let listing = matches!(served_by, "session-transcript" | "session-since");
    if !listing || text.len() <= LISTING_BYTES {
        return text;
    }
    let total = text.lines().count();
    let (mut kept, mut used) = (0usize, 0usize);
    for line in text.lines() {
        let next = used
            .saturating_add(line.len())
            .saturating_add(usize::from(kept > 0));
        if next > LISTING_BYTES && kept > 0 {
            break;
        }
        (kept, used) = (kept.saturating_add(1), next);
    }
    let shown = text.lines().take(kept).collect::<Vec<_>>().join("\n");
    let cut = shown.floor_char_boundary(LISTING_BYTES);
    let clipped = if cut < shown.len() {
        " (the last one clipped)"
    } else {
        ""
    };
    format!(
        "{}\n[… {kept} of {total} entries shown{clipped} · unpaged history cap {LISTING_BYTES} bytes · read {url} with offset={} and limit to page on]",
        shown.get(..cut).unwrap_or_default(),
        kept.saturating_add(1)
    )
}

/// Incident: a fact past the cut of a compact line took a child four guessed offsets before it
/// read the entry whole.
fn compact_note(url: &Url, served_by: &str, text: String) -> String {
    let compact = matches!(served_by, "session-since" | "session-tail");
    if !compact || !text.lines().any(|line| line.ends_with('…')) {
        return text;
    }
    let full = url.to_string();
    let base = ["/since/", "/tail/"]
        .iter()
        .find_map(|window| full.split_once(window).map(|(base, _)| base))
        .unwrap_or(&full);
    let (note, user) = (crate::family::NOTE_CHARS, crate::family::USER_CHARS);
    format!(
        "{text}\n[… a line ending in … is cut at {note} chars (NOTE_CHARS), a user line at {user} (USER_CHARS) · read {base} with offset=<#seq> and limit=1 for that entry whole]"
    )
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
                None => {
                    let text = capped(&fetched.url, &fetched.served_by, fetched.text);
                    text_output(compact_note(&fetched.url, &fetched.served_by, text))
                }
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
