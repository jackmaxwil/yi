/// `login` and `logout` carry flags of their own (`--key`, `--no-browser`) that
/// `parse_args` would refuse as unexpected, so they answer before it like `mcp` does.
pub(crate) fn fast_path() {
    let verb = std::env::args().nth(1);
    let Some(verb) = verb
        .as_deref()
        .filter(|verb| matches!(*verb, "login" | "logout"))
    else {
        return;
    };
    let rest: Vec<String> = std::env::args().skip(2).collect();
    std::process::exit(match verb {
        "login" => refreshed(yi_runtime::auth::login(&rest), &rest),
        _ => yi_runtime::auth::logout(&rest),
    });
}

/// A provider whose model list is the account's own is refreshed on a login that worked, so the
/// first `--model <provider>/<id>` after it resolves; the ids it lists are printed.
fn refreshed(code: i32, args: &[String]) -> i32 {
    let provider = args.iter().find(|word| !word.starts_with('-'));
    let (0, Some(provider), Some(dir)) = (code, provider, yi_runtime::catalog_cache_dir()) else {
        return code;
    };
    if args.iter().any(|word| word == "--list") || !yi_runtime::catalog_needs_credential(provider) {
        return code;
    }
    let key = crate::yi_ai_key(provider);
    let fetched = crate::proxy_from_env().and_then(|proxy| {
        yi_runtime::refresh_catalog(
            dir,
            provider,
            key.as_ref().map(|k| k.expose()),
            proxy.as_ref(),
        )
    });
    match fetched {
        Ok(_) => yi_runtime::available_models()
            .iter()
            .filter(|model| model.provider == *provider)
            .for_each(|model| println!("{provider}/{}", model.id)),
        Err(error) => eprintln!(
            "warning: logged in, but the {provider} model list did not refresh: {error}; run: yi catalog refresh {provider}"
        ),
    }
    code
}

/// A provider with neither an env key nor a stored credential: the refusal names
/// the verb that fixes it, so a missing login never reads as a provider outage.
pub(crate) fn no_credential(provider: &str) -> crate::Refused {
    crate::Refused {
        code: 4,
        reason: yi_runtime::auth::missing_message(provider),
        class: yi_types::telemetry::ErrorClass::RefusalNoKey,
    }
}
