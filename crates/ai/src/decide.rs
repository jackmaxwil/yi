use std::time::Duration;

use yi_types::classifier::{DecisionRequest, DecisionResponse};

const BODY_CAP: u64 = 64 * 1024;

pub fn decide(
    base_url: &str,
    key: Option<&str>,
    deadline: Duration,
    request: &DecisionRequest,
) -> Result<DecisionResponse, String> {
    let url = format!("{}/v1/systemone", base_url.trim_end_matches('/'));
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    let agent = ureq::AgentBuilder::new().timeout(deadline).build();
    let mut call = agent.post(&url).set("content-type", "application/json");
    if let Some(key) = key {
        call = call.set("authorization", &format!("Bearer {key}"));
    }
    let response = call.send_string(&body).map_err(|error| match error {
        ureq::Error::Status(code, _) => format!("{url} answered HTTP {code}"),
        ureq::Error::Transport(transport) if transport.url().is_some() => transport.to_string(),
        ureq::Error::Transport(transport) => format!("{url}: {transport}"),
    })?;
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(response.into_reader(), BODY_CAP),
        &mut text,
    )
    .map_err(|error| format!("{url}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{url}: not a decision: {error}"))
}
