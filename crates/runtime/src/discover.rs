use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use yi_ai::catalog::Catalog;
use yi_types::model::Model;

static CUSTOM_PROVIDERS: OnceLock<yi_types::config::Providers> = OnceLock::new();

pub(crate) fn install(providers: &yi_types::config::Providers) {
    let _ = CUSTOM_PROVIDERS.set(providers.clone());
}
static DISCOVERED_MODELS: LazyLock<Mutex<BTreeMap<String, Discovery>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static DISCOVERY_WAKE: LazyLock<Condvar> = LazyLock::new(Condvar::new);
const DISCOVERY_TTL: Duration = Duration::from_secs(30);

enum Discovery {
    Pending {
        last: Option<Arc<Vec<Model>>>,
    },
    Ready {
        models: Arc<Vec<Model>>,
    },
    Failed {
        error: String,
        at: Instant,
        last: Option<Arc<Vec<Model>>>,
    },
}

/// OpenRouter's model list omits routing variants (`:exacto`, `:nitro`): one takes its base's entry.
pub fn resolve_model(provider: &str, id: &str) -> Option<Model> {
    resolve_in(Catalog::shared(), provider, id).or_else(|| {
        let Some(Ok(models)) = cached(provider) else {
            return None;
        };
        models.iter().find(|model| model.id == id).cloned()
    })
}

pub fn select_model(provider: &str, id: &str) -> Result<Model, String> {
    if let Some(model) = resolve_model(provider, id) {
        return Ok(model);
    }
    if !dynamic_provider(provider) {
        return Err(format!("unknown model {provider}/{id} (use provider/id)"));
    }
    discover_models(provider)?
        .iter()
        .find(|model| model.id == id)
        .cloned()
        .ok_or_else(|| format!("unknown model {provider}/{id} (use provider/id)"))
}

fn dynamic_provider(id: &str) -> bool {
    CUSTOM_PROVIDERS.get().is_some_and(|providers| {
        providers
            .iter()
            .any(|(provider_id, provider)| provider_id == id && provider.models.is_empty())
    })
}

/// A Cursor id is taken on trust only until the account's model list has been fetched: with a
/// list on disk a typo resolves to nothing, as for every other provider.
pub(crate) fn resolve_in(catalog: &Catalog, provider: &str, id: &str) -> Option<Model> {
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
    let mut models = Catalog::shared().models();
    let Some(providers) = CUSTOM_PROVIDERS.get() else {
        return models;
    };
    for (id, provider) in providers.iter() {
        if provider.models.is_empty()
            && let Ok(discovered) = discover_models(id)
        {
            models.extend(discovered.iter().cloned());
        }
    }
    models
}

pub fn listed_models() -> Vec<Model> {
    available_models()
}

pub fn dynamic_model_count(id: &str) -> Result<usize, String> {
    Ok(discover_models(id)?.len())
}

pub fn proxy_from_env() -> Result<Option<yi_ai::request::ProxyConfig>, String> {
    let var = |name: &str, fallback: &str| {
        std::env::var(name)
            .or_else(|_| std::env::var(fallback))
            .ok()
    };
    yi_ai::request::ProxyConfig::from_values(
        var("HTTPS_PROXY", "https_proxy").as_deref(),
        var("HTTP_PROXY", "http_proxy").as_deref(),
        var("NO_PROXY", "no_proxy").as_deref(),
    )
}

fn discover_models(id: &str) -> Result<Arc<Vec<Model>>, String> {
    let providers = CUSTOM_PROVIDERS
        .get()
        .ok_or("custom providers are not configured")?;
    let provider = providers
        .iter()
        .find(|(provider_id, _)| *provider_id == id)
        .map(|(_, provider)| provider)
        .ok_or("unknown custom provider")?;
    if !provider.models.is_empty() {
        return Ok(Arc::new(provider.catalog(id)));
    }
    loop {
        let mut cache = DISCOVERED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match remembered(cache.get(id)) {
            Memory::Fresh(result) => return result,
            Memory::Pending => {
                let _waiting = DISCOVERY_WAKE
                    .wait(cache)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            Memory::Miss => {
                let last = match cache.get(id) {
                    Some(Discovery::Ready { models, .. }) => Some(Arc::clone(models)),
                    Some(Discovery::Failed { last, .. }) | Some(Discovery::Pending { last }) => {
                        last.clone()
                    }
                    None => None,
                };
                cache.insert(id.to_owned(), Discovery::Pending { last });
                break;
            }
        }
    }
    let outcome = fetch_models(id, provider).map(Arc::new);
    let last = {
        let cache = DISCOVERED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match cache.get(id) {
            Some(Discovery::Pending { last }) => last.clone(),
            _ => None,
        }
    };
    let found = match &outcome {
        Ok(models) => Discovery::Ready {
            models: Arc::clone(models),
        },
        Err(error) => Discovery::Failed {
            error: error.clone(),
            at: Instant::now(),
            last,
        },
    };
    DISCOVERED_MODELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id.to_owned(), found);
    DISCOVERY_WAKE.notify_all();
    outcome
}

fn fetch_models(
    id: &str,
    provider: &yi_types::config::CustomProvider,
) -> Result<Vec<Model>, String> {
    let proxy = proxy_from_env()?;
    let bearer =
        yi_ai::auth::resolve(id).map(|credential| format!("Bearer {}", credential.secret.expose()));
    let headers: Vec<(&str, String)> = bearer
        .iter()
        .map(|value| ("Authorization", value.clone()))
        .collect();
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    let response = yi_ai::request::get_json(&url, &headers, proxy.as_ref(), 8 * 1024 * 1024)?;
    let Some(data) = response.get("data").and_then(serde_json::Value::as_array) else {
        return Err(format!("{url}: model list has no data array"));
    };
    let list = yi_types::config::CustomModelList {
        data: data
            .iter()
            .filter_map(|value| serde_json::from_value(value.clone()).ok())
            .collect(),
    };
    let models = provider.discovered_catalog(id, list);
    if models.is_empty() && !data.is_empty() {
        return Err(format!("{url}: model list named no usable model"));
    }
    Ok(models)
}

enum Memory {
    Fresh(Result<Arc<Vec<Model>>, String>),
    Pending,
    Miss,
}

fn remembered(found: Option<&Discovery>) -> Memory {
    match found {
        Some(Discovery::Ready { models, .. }) => Memory::Fresh(Ok(Arc::clone(models))),
        Some(Discovery::Failed { error, at, .. }) if at.elapsed() < DISCOVERY_TTL => {
            Memory::Fresh(Err(error.clone()))
        }
        Some(Discovery::Failed { .. }) => Memory::Miss,
        Some(Discovery::Pending { .. }) => Memory::Pending,
        None => Memory::Miss,
    }
}

fn cached(id: &str) -> Option<Result<Arc<Vec<Model>>, String>> {
    let Ok(cache) = DISCOVERED_MODELS.lock() else {
        return None;
    };
    match cache.get(id)? {
        Discovery::Ready { models, .. }
        | Discovery::Pending { last: Some(models) }
        | Discovery::Failed {
            last: Some(models), ..
        } => Some(Ok(Arc::clone(models))),
        Discovery::Failed {
            error,
            at,
            last: None,
        } if at.elapsed() < DISCOVERY_TTL => Some(Err(error.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_discovery_is_replayed_until_it_ages_out() {
        let id = "ttl-probe";
        let mut cache = DISCOVERED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.insert(
            id.to_owned(),
            Discovery::Failed {
                error: "down".to_owned(),
                at: Instant::now(),
                last: None,
            },
        );
        drop(cache);
        let message = match cached(id) {
            Some(Err(error)) => error,
            _ => String::new(),
        };
        assert_eq!(message, "down");
        let mut cache = DISCOVERED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.insert(
            id.to_owned(),
            Discovery::Failed {
                error: "down".to_owned(),
                at: Instant::now()
                    .checked_sub(Duration::from_secs(31))
                    .unwrap_or_else(Instant::now),
                last: None,
            },
        );
        drop(cache);
        assert!(cached(id).is_none(), "an aged failure is fetched again");
        let mut cache = DISCOVERED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.insert(
            id.to_owned(),
            Discovery::Ready {
                models: Arc::new(Vec::new()),
            },
        );
        drop(cache);
        assert!(cached(id).is_some(), "a stale list still resolves");
    }

    #[test]
    fn two_callers_share_one_models_fetch() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&hits);
        let release = std::sync::Arc::new(AtomicBool::new(false));
        let opened = std::sync::Arc::clone(&release);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let mut got = Vec::new();
                let mut byte = [0u8; 1];
                while got.windows(4).last() != Some(b"\r\n\r\n") && got.len() < 4096 {
                    if stream.read(&mut byte).ok() != Some(1) {
                        break;
                    }
                    got.push(byte[0]);
                }
                while !opened.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let body = r#"{"data":[{"id":"m2"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let raw = format!(
            r#"{{"providers":{{"sharegw":{{"baseUrl":"http://127.0.0.1:{port}/v1","models":[]}}}}}}"#
        );
        let (config, _) = yi_types::config::parse(&raw).unwrap();
        install(config.providers.as_ref().unwrap());
        let (send_first, first) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send_first.send(dynamic_model_count("sharegw"));
        });
        for _ in 0..100 {
            if hits.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (send_second, second) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send_second.send(dynamic_model_count("sharegw"));
        });
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a second caller opened its own fetch"
        );
        release.store(true, Ordering::SeqCst);
        let left = first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let right = second
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!((left, right), (1, 1));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
