//! The model registry as a spawn sees it: the `rlm.find_models` listing.

use serde_json::{Map, Value, json};
use yi_types::model::Model;

use super::SubagentHost;
use crate::provider::{ProviderStream, available_models};

impl SubagentHost {
    pub fn find_models(&self, query: &str, limit: usize) -> Map<String, Value> {
        let needle = query.to_lowercase();
        let matched: Vec<Model> = credentialed_models(&self.options.provider)
            .into_iter()
            .filter(|model| {
                needle.is_empty()
                    || model.id.to_lowercase().contains(&needle)
                    || model.name.to_lowercase().contains(&needle)
                    || model.provider.to_lowercase().contains(&needle)
            })
            .collect();
        let total = matched.len();
        let models: Vec<Value> = matched
            .into_iter()
            .take(limit)
            .map(|model| {
                let selector = selector_of(&model);
                let mut row = json!({
                    "provider": model.provider,
                    "id": model.id,
                    "name": model.name,
                    "selector": selector,
                    "cost": {"input": model.cost.input, "output": model.cost.output},
                });
                if let Some(cause) = self.failing_route(&selector) {
                    row["failing"] = Value::String(format!(
                        "its last child died on its first model call: {cause}"
                    ));
                }
                row
            })
            .collect();
        let mut reply = Map::new();
        reply.insert("models".to_owned(), Value::Array(models));
        if total > limit {
            let query = Value::from(query);
            let next = format!("await rlm.find_models({query}, limit={total})");
            let row =
                crate::listed::cut_row(limit, total, "matching models", "limit", limit, &next);
            reply.insert("cut".to_owned(), Value::String(row));
        }
        reply
    }
}

/// The registry's models whose provider holds a credential, each provider asked once.
pub fn credentialed_models(stream: &ProviderStream) -> Vec<Model> {
    let mut asked = std::collections::HashMap::new();
    let mut models = available_models();
    models.retain(|model| {
        *asked
            .entry(model.provider.clone())
            .or_insert_with(|| stream.has_credential(&model.provider))
    });
    models
}

pub fn selector_of(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

const ROUTE_FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(600);
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
const FAILURE_SHOWN_CHARS: usize = 200;

impl SubagentHost {
    pub fn note_route(&self, selector: &str, failure: Option<&str>) {
        let Ok(mut routes) = self.routes.lock() else {
            return;
        };
        match failure {
            Some(cause) => {
                let shown: String = cause.chars().take(FAILURE_SHOWN_CHARS).collect();
                routes.insert(selector.to_owned(), (std::time::Instant::now(), shown));
            }
            None => {
                routes.remove(selector);
            }
        }
    }

    pub fn failing_route(&self, selector: &str) -> Option<String> {
        let routes = self.routes.lock().ok()?;
        let (when, cause) = routes.get(selector)?;
        (when.elapsed() < ROUTE_FAILURE_TTL).then(|| cause.clone())
    }
}

/// A loopback `base_url` whose port refuses a connection: the one route failure that can be
/// told before a child is spawned, with no key and no cost. Anything else answers `None`.
pub(crate) fn loopback_refused(base_url: &str) -> Option<String> {
    use std::net::ToSocketAddrs;
    let (scheme, rest) = base_url.split_once("://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || authority.contains("]:") => {
            (host, port.parse::<u16>().ok()?)
        }
        _ => (authority, if scheme == "https" { 443 } else { 80 }),
    };
    if port == 0 {
        return None;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if !matches!(bare, "127.0.0.1" | "localhost" | "::1") {
        return None;
    }
    let addr = (bare, port).to_socket_addrs().ok()?.next()?;
    match std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            Some(format!("{base_url} refused a connection"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::loopback_refused;
    use std::net::TcpListener;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_closed_loopback_port_is_refused_and_an_open_one_is_not() -> TestResult {
        let open = TcpListener::bind("127.0.0.1:0")?;
        let port = open.local_addr()?.port();
        assert_eq!(
            loopback_refused(&format!("http://127.0.0.1:{port}/v1")),
            None
        );
        drop(open);
        let refused = loopback_refused(&format!("http://127.0.0.1:{port}/v1"));
        assert_eq!(
            refused,
            Some(format!("http://127.0.0.1:{port}/v1 refused a connection"))
        );
        Ok(())
    }

    #[test]
    fn a_remote_or_unparsable_base_url_is_never_probed() {
        assert_eq!(loopback_refused("https://openrouter.ai/api/v1"), None);
        assert_eq!(loopback_refused("not a url"), None);
        assert_eq!(loopback_refused(""), None);
        assert_eq!(loopback_refused("http://localhost:0"), None);
    }
}
