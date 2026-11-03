//! The advisor's wiring: its hold sink, its review model and the rules directory.

use std::sync::Arc;

use crate::session::AgentSession;
use crate::wiring::RuntimeWiring;

pub(crate) fn attach(session: &AgentSession, wiring: &RuntimeWiring) {
    let hold_sink: Option<crate::advisor::HoldSink> = wiring.broker.as_ref().map(|broker| {
        let broker = Arc::clone(broker);
        Arc::new(move |advice: &yi_types::advisor::Advice| {
            let pattern = advice
                .target
                .as_deref()
                .and_then(yi_permission::HoldPattern::new);
            let Some(pattern) = pattern.filter(|_| broker.can_ask()) else {
                return false;
            };
            broker.insert_hold(yi_permission::Hold {
                pattern,
                reason: advice.text.clone(),
                source: yi_permission::HoldSource::Advisor,
                expires_at_ms: Some(yi_session::now_ms().saturating_add(3_600_000)),
            });
            true
        }) as crate::advisor::HoldSink
    });
    // §16: ADVISOR.md attention text, project-local, best-effort.
    let attention = std::fs::read_to_string(wiring.cwd.join("ADVISOR.md")).ok();
    let llm = wiring.advisor.clone().map(|model| {
        // Its own conversation: the root's ledger estimate and hour prefix are not its (D315).
        Arc::new(crate::advisor::review::LlmReviewer::new(
            Arc::new(wiring.provider.for_child()),
            model,
            attention.clone(),
        ))
    });
    let advisor = crate::advisor::attach_advisor(
        session,
        crate::advisor::AdvisorConfig {
            attention,
            reviewer: llm.is_some(),
            rules_dir: Some(wiring.cwd.join(".yi/rules")),
            ..crate::advisor::AdvisorConfig::default()
        },
        crate::advisor::AdvisorDeps { hold_sink, llm },
    );
    session.set_advisor(advisor);
}
