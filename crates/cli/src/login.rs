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

/// A provider with neither an env key nor a stored credential: the refusal names
/// the verb that fixes it, so a missing login never reads as a provider outage.
pub(crate) fn no_credential(provider: &str) -> crate::Refused {
    crate::Refused {
        code: 4,
        reason: yi_runtime::auth::missing_message(provider),
        class: yi_types::telemetry::ErrorClass::RefusalNoKey,
    }
}
