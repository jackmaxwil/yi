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
