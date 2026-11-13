use yi_types::model::Model;

/// §5: an unset role falls back to the primary; an unknown one warns, naming what happens instead.
pub(crate) fn role_model(role: &str, spec: Option<String>, instead: &str) -> Option<Model> {
    let spec = spec?;
    match crate::resolve(&spec) {
        Ok(model) => Some(model),
        Err(error) => {
            if error.starts_with("unknown model") {
                eprintln!("warning: unknown {role} model {spec}; {instead}");
            } else {
                eprintln!("warning: {role} model {spec}: {error}; {instead}");
            }
            None
        }
    }
}

/// Naming `models.advisor` is what turns the LLM reviewer on (D28/D50).
pub(crate) fn advisor_model() -> Option<Model> {
    let spec = crate::configured_roles().advisor;
    role_model("advisor", spec, "the advisor stays silent")
}

/// Naming `models.autoReview` is the switch for the §8 permission reviewer (D81).
pub(crate) fn auto_review_model() -> Option<Model> {
    let spec = crate::configured_roles().auto_review;
    role_model("autoReview", spec, "the auto reviewer stays off")
}

pub(crate) fn summarizer_model(args: &crate::Args) -> Option<Model> {
    let spec = crate::configured_roles().summarizer;
    role_model("summarizer", spec, &format!("using {}", args.model))
}

/// Naming `models.selector` turns on line selection for cut bash output (stage 9).
pub(crate) fn selector_model() -> Option<Model> {
    let spec = crate::configured_roles().selector;
    role_model("selector", spec, "the reducer's cut stands alone")
}

pub(crate) fn web_search_model() -> Option<Model> {
    let spec = crate::configured_roles()
        .web_search
        .unwrap_or_else(|| "openrouter/deepseek/deepseek-v4-flash".to_owned());
    role_model("webSearch", Some(spec), "web search stays off")
}

pub(crate) fn web_search_provider(
    proxy: Option<yi_runtime::ProxyConfig>,
) -> Result<Option<std::sync::Arc<dyn yi_runtime::search::SearchProvider>>, String> {
    build_web_search(
        crate::configured_roles().web_search,
        web_search_model,
        crate::config()
            .web_search
            .as_ref()
            .and_then(|search| search.engine.clone()),
        yi_runtime::auth::resolve_with_proxy("openrouter", proxy.as_ref())
            .map(|resolved| resolved.secret),
        proxy,
        crate::config().routing.clone(),
    )
}

fn build_web_search(
    configured: Option<String>,
    model: impl FnOnce() -> Option<Model>,
    engine: Option<String>,
    credential: Option<yi_runtime::auth::Secret>,
    proxy: Option<yi_runtime::ProxyConfig>,
    routing: Option<serde_json::Value>,
) -> Result<Option<std::sync::Arc<dyn yi_runtime::search::SearchProvider>>, String> {
    let Some(credential) = credential else {
        return Ok(None);
    };
    let Some(model) = model() else {
        return Ok(None);
    };
    if let Some(spec) = configured.filter(|_| model.provider != "openrouter") {
        return Err(format!(
            "models.webSearch names {spec}, which is not OpenRouter; choose an OpenRouter model or remove the setting"
        ));
    }
    let engine = engine
        .map_or_else(
            || Ok(yi_runtime::search::SearchEngine::default()),
            yi_runtime::search::SearchEngine::new,
        )
        .map_err(|error| format!("{error}; choose a non-empty engine or remove the setting"))?;
    let provider =
        yi_runtime::search::OpenRouterSearch::new(model, Some(credential), engine, proxy, routing)
            .map_err(|error| error.to_string())?;
    Ok(Some(std::sync::Arc::new(provider)))
}

#[cfg(test)]
mod tests {
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn web_search_validation_waits_for_credential_and_role_gates() -> TestResult {
        let search = super::build_web_search(
            Some("faux/faux-1".to_owned()),
            || crate::resolve("faux/faux-1").ok(),
            Some(String::new()),
            None,
            None,
            None,
        )?;
        assert!(search.is_none());

        let search = super::build_web_search(
            None,
            || None,
            Some(String::new()),
            Some(yi_runtime::auth::Secret::new("x".to_owned())),
            None,
            None,
        )?;
        assert!(search.is_none());
        Ok(())
    }
}
