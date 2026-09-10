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
        "login" => yi_runtime::auth::login(&rest),
        _ => yi_runtime::auth::logout(&rest),
    });
}

/// The credential the session streams with: the secret plus, for a stored OAuth
/// login, its profile's headers (D172). `None` is the faux provider, which needs none.
pub(crate) fn stream_for(
    resolved: Option<&yi_runtime::auth::Resolved>,
) -> yi_runtime::ProviderStream {
    let secret =
        resolved.map(|found| yi_runtime::auth::Secret::new(found.secret.expose().to_owned()));
    let stream = yi_runtime::ProviderStream::new(secret, None);
    match resolved {
        Some(found) => stream.with_auth(found),
        None => stream,
    }
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
