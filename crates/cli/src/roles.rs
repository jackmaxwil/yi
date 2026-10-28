use yi_types::model::Model;

/// §5: an unset role falls back to the primary; an unknown one warns, naming what happens instead.
pub(crate) fn role_model(role: &str, spec: Option<String>, instead: &str) -> Option<Model> {
    let spec = spec?;
    let resolved = crate::resolve(&spec);
    if resolved.is_none() {
        eprintln!("warning: unknown {role} model {spec}; {instead}");
    }
    resolved
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
    let Some(model) = web_search_model() else {
        return Ok(None);
    };
    let Some(credential) = yi_runtime::auth::resolve_with_proxy("openrouter", proxy.as_ref())
    else {
        return Ok(None);
    };
    let configured = crate::config()
        .web_search
        .as_ref()
        .and_then(|search| search.engine.clone());
    let provider = configured
        .map_or_else(
            || Ok(yi_runtime::search::SearchEngine::default()),
            yi_runtime::search::SearchEngine::new,
        )
        .and_then(|engine| {
            yi_runtime::search::OpenRouterSearch::new(
                model,
                Some(credential.secret),
                engine,
                proxy,
                crate::config().routing.clone(),
            )
        });
    match provider {
        Ok(provider) => Ok(Some(std::sync::Arc::new(provider))),
        Err(error) => {
            eprintln!("{error}; web search stays off");
            Ok(None)
        }
    }
}
