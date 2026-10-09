use yi_types::url::Url;

use super::{FetchError, Resolver};

type Guide = fn() -> String;

/// Invariant: every tool whose description names `yi://tools/<name>` has a row here; the
/// request-budget test reads each pointer back through the session's own `read`.
const TOOL_GUIDES: [(&str, Guide); 4] = [
    ("edit", || yi_tools::hashline::tool::EDIT_GUIDE.to_owned()),
    ("plan", plan_guide),
    ("todo", || crate::todo::tool::GUIDE.to_owned()),
    ("ask_user", || crate::auto_review::ASK_USER_GUIDE.to_owned()),
];

fn plan_guide() -> String {
    let fields = serde_json::to_string_pretty(&crate::plan::tool::schema()).unwrap_or_default();
    format!(
        "{}\n\nEvery parameter, documented:\n{fields}",
        crate::plan::tool::GUIDE
    )
}

impl Resolver {
    /// `yi://tools/<name>` serves a deferred tool guide, `yi://skills/<name>` a skill's
    /// SKILL.md: the documents the prefix names but does not carry.
    pub(super) fn resolve_docs(&self, url: &Url) -> Result<(String, String), FetchError> {
        let not_found = |what: String| FetchError::NotFound {
            url: url.to_string(),
            what,
        };
        match url.path().split_once('/') {
            Some(("tools", name)) => TOOL_GUIDES
                .iter()
                .find(|(tool, _)| *tool == name)
                .map(|(_, guide)| (guide(), "tool-guide".to_owned()))
                .ok_or_else(|| {
                    let names: Vec<&str> = TOOL_GUIDES.iter().map(|(tool, _)| *tool).collect();
                    not_found(format!(
                        "a guide for {name}; guides exist for {}, and every other tool's description is whole",
                        names.join(", ")
                    ))
                }),
            Some(("skills", name)) => {
                let skills = crate::skills::discover(self.workspace(), self.home());
                let skill = skills
                    .iter()
                    .find(|skill| skill.name == name)
                    .ok_or_else(|| not_found(format!("skill {name}")))?;
                super::read_text(url, &skill.path, self.workspace(), &self.wall().deny_read)
                    .map(|text| (text, "skill".to_owned()))
            }
            _ => Err(FetchError::BadAddress {
                url: url.to_string(),
                detail: "a yi address is yi://tools/<name> or yi://skills/<name>".to_owned(),
            }),
        }
    }
}
