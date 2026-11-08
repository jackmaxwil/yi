use yi_types::event::{AgentEvent, RunEnd};
use yi_types::message::AgentMessage;

use crate::config::LoopConfig;

pub(crate) async fn side_work(config: &LoopConfig) {
    if let Some(gate) = &config.side_work {
        gate().await;
    }
}

/// Closes a run: the side work it owes the host, then the one `agent_end` carrying why it stopped.
pub(crate) async fn end(
    config: &LoopConfig,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
    collected: Vec<AgentMessage>,
    reason: RunEnd,
) -> Vec<AgentMessage> {
    side_work(config).await;
    emit(AgentEvent::AgentEnd {
        messages: collected.clone(),
        reason: Some(reason),
    });
    collected
}
