/// Past this a pooled connection is not trusted: a NAT may have dropped it without a FIN.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(30);

static POOLED: std::sync::Mutex<Vec<Pooled>> = std::sync::Mutex::new(Vec::new());

/// A deadline is built into the agent, so a pool keyed by nothing but idle time handed a fast
/// model's connection the slow model's bound. One slot thrashed two models in one session.
struct Pooled {
    agent: ureq::Agent,
    idle_since: std::time::Instant,
    read_timeout: std::time::Duration,
    /// A finished stream handed a socket back to this agent's pool; before that, a held agent
    /// has dialed nothing that could have gone stale.
    warm: bool,
}

// ponytail: a proxied request pools nothing; key the pool by proxy if that path matters.
/// `fresh` drops the held slot first; the flag returned says the agent holds a pooled socket.
pub(super) fn stream_agent(
    proxy: Option<&ureq::Proxy>,
    read_timeout: std::time::Duration,
    fresh: bool,
) -> (ureq::Agent, bool) {
    let builder = ureq::AgentBuilder::new()
        .user_agent(yi_types::USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(read_timeout)
        // ureq keeps one idle connection per host by default, so a later turn of the session
        // dialed again: the family's eight live streams is the width worth keeping.
        .max_idle_connections(16)
        .max_idle_connections_per_host(16);
    if let Some(proxy) = proxy {
        return (builder.proxy(proxy.clone()).build(), false);
    }
    let Ok(mut pooled) = POOLED.lock() else {
        return (builder.build(), false);
    };
    pooled.retain(|held| {
        held.idle_since.elapsed() < KEEP_ALIVE && !(fresh && held.read_timeout == read_timeout)
    });
    if let Some(agent) = pooled
        .iter_mut()
        .find(|held| held.read_timeout == read_timeout)
        .map(|held| {
            held.idle_since = std::time::Instant::now();
            (held.agent.clone(), held.warm)
        })
    {
        return agent;
    }
    let agent = builder.build();
    if pooled.len() >= POOLED_SLOTS
        && let Some((at, _)) = pooled
            .iter()
            .enumerate()
            .min_by_key(|(_, held)| held.idle_since)
    {
        drop(pooled.swap_remove(at));
    }
    pooled.push(Pooled {
        agent: agent.clone(),
        idle_since: std::time::Instant::now(),
        read_timeout,
        warm: false,
    });
    (agent, false)
}

/// Two deadlines at once is the alternating main/advisor pattern; a third covers a subagent.
const POOLED_SLOTS: usize = 3;

pub(super) fn pooled_idle_from_now(read_timeout: std::time::Duration) {
    if let Ok(mut pooled) = POOLED.lock()
        && let Some(held) = pooled
            .iter_mut()
            .find(|held| held.read_timeout == read_timeout)
    {
        held.idle_since = std::time::Instant::now();
        held.warm = true;
    }
}

/// Incident: ureq clears the read deadline when a socket returns to its pool, so a request
/// written into one a NAT dropped waited forever; `None` is a socket silent past `wait`.
pub(super) fn send_on_pooled(
    request: ureq::Request,
    body: &str,
    wait: std::time::Duration,
) -> Option<Result<ureq::Response, ureq::Error>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let _ = sender.send(request.send_string(&body));
    });
    receiver.recv_timeout(wait).ok()
}
