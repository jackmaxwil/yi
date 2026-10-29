use std::sync::OnceLock;
use std::time::Duration;
use yi_types::classifier::{DecisionRequest, DecisionResponse};

const BODY_CAP: u64 = 64 * 1024;

/// Invariant: an error is the cause alone; the journal, the pause notice and doctor each prefix
/// the endpoint's URL once.
pub fn decide(
    base_url: &str,
    key: Option<&str>,
    deadline: Duration,
    request: &DecisionRequest,
) -> Result<DecisionResponse, String> {
    let url = format!("{}/v1/systemone", base_url.trim_end_matches('/'));
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    let mut call = agent()
        .post(&url)
        .timeout(deadline)
        .set("content-type", "application/json");
    if let Some(key) = key {
        call = call.set("authorization", &format!("Bearer {key}"));
    }
    let response = call.send_string(&body).map_err(|error| match error {
        ureq::Error::Status(code, _) => format!("the classifier answered HTTP {code}"),
        ureq::Error::Transport(transport) => crate::request::transport_cause(&transport),
    })?;
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(response.into_reader(), BODY_CAP),
        &mut text,
    )
    .map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| format!("not a decision: {error}"))
}

/// Incident: the endpoint moved from loopback to a remote HTTPS host, so a per-call agent paid a
/// DNS lookup and a TLS handshake on every judged command.
fn agent() -> &'static ureq::Agent {
    static SHARED: OnceLock<ureq::Agent> = OnceLock::new();
    SHARED.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(yi_types::USER_AGENT)
            .max_idle_connections(8)
            .build()
    })
}
