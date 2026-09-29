//! `yi catalog`, and the background refresh a session starts (D128).

use crate::{Args, config, proxy_from_env, yi_ai_key};

pub(crate) fn refresh_hours(catalog: Option<&yi_types::config::CatalogConfig>) -> Option<u64> {
    if catalog.and_then(|c| c.enabled) == Some(false) {
        return None;
    }
    Some(
        catalog
            .and_then(|c| c.refresh_hours)
            .unwrap_or(yi_runtime::DEFAULT_REFRESH_HOURS),
    )
}

pub(crate) fn clock() -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_millis(yi_runtime::session_store::now_ms())
}

fn walled(provider: &str) -> Option<String> {
    let wall = yi_runtime::Wall::default();
    let workspace = std::env::current_dir().unwrap_or_default();
    [yi_runtime::MODELS_DEV]
        .into_iter()
        .chain(yi_runtime::catalog_list_url(provider))
        .filter_map(|target| target.parse::<yi_types::url::Url>().ok())
        .find_map(|url| wall.check_url(&url, &workspace))
}

/// A refresh runs beside the session, never in it: the file it writes is read by the next
/// process, so a slow or failing fetch costs this turn nothing.
pub(crate) fn spawn_refresh(
    provider: &str,
    key: Option<&yi_runtime::auth::Secret>,
    proxy: Option<&yi_runtime::ProxyConfig>,
) {
    let Some(hours) = refresh_hours(config().catalog.as_ref()) else {
        return;
    };
    let Some(dir) = yi_runtime::catalog_cache_dir() else {
        return;
    };
    if !yi_runtime::catalog_is_stale(dir, provider, hours, clock()) || walled(provider).is_some() {
        return;
    }
    let provider = provider.to_owned();
    let key = key.map(|secret| secret.expose().to_owned());
    let proxy = proxy.cloned();
    std::thread::spawn(move || {
        let _next_session_reads_it =
            yi_runtime::refresh_catalog(dir, &provider, key.as_deref(), proxy.as_ref());
    });
}

pub(crate) fn run(args: &Args) -> i32 {
    let Some(dir) = yi_runtime::catalog_cache_dir() else {
        eprintln!("error: no HOME, so no ~/.yi/catalog");
        return 2;
    };
    let mut words = args.prompt.split_whitespace();
    let width = yi_runtime::CATALOG_PROVIDERS
        .iter()
        .map(|provider| provider.len())
        .max()
        .unwrap_or_default();
    match words.next() {
        None => {
            for provider in yi_runtime::CATALOG_PROVIDERS {
                let age = yi_runtime::catalog_age(dir, provider, clock()).map_or_else(
                    || "bundled only".to_owned(),
                    |age| format!("{}h old", age.as_secs() / 3600),
                );
                println!("{provider:<width$} {age}");
            }
            0
        }
        Some("refresh") => {
            let only = words.next();
            let proxy = match proxy_from_env() {
                Ok(proxy) => proxy,
                Err(message) => {
                    eprintln!("error: {message}");
                    return 2;
                }
            };
            let mut code = 0;
            for provider in yi_runtime::CATALOG_PROVIDERS {
                if only.is_some_and(|name| name != provider) {
                    continue;
                }
                // A provider with no list adapter is bundled by design; there is
                // nothing to fetch, and the run has not failed.
                if yi_runtime::catalog_list_url(provider).is_none() {
                    println!("{provider:<width$} bundled only");
                    continue;
                }
                if let Some(refusal) = walled(provider) {
                    eprintln!("{provider:<width$} {refusal}");
                    code = 1;
                    continue;
                }
                let key = yi_ai_key(provider);
                match yi_runtime::refresh_catalog(
                    dir,
                    provider,
                    key.as_ref().map(|k| k.expose()),
                    proxy.as_ref(),
                ) {
                    Ok(count) => println!("{provider:<width$} {count} models"),
                    Err(error) => {
                        eprintln!("{provider:<width$} {error}");
                        code = 1;
                    }
                }
            }
            code
        }
        Some(other) => {
            eprintln!("usage: yi catalog [refresh [provider]] (not {other:?})");
            2
        }
    }
}
