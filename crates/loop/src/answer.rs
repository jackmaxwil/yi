use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::AgentMessage;

use crate::LoopContext;

/// Invariant: clients print an answer from its text deltas, so an answer the model wrote as a
/// tool argument is replayed as one streamed text block before its end.
pub(crate) fn push(
    answer: AgentMessage,
    context: &mut LoopContext,
    collected: &mut Vec<AgentMessage>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) {
    let mut partial = answer.clone();
    let text = match &mut partial {
        AgentMessage::Assistant { content, .. } => {
            yi_types::message::join_text(&std::mem::take(content), "")
        }
        _ => String::new(),
    };
    emit(AgentEvent::MessageStart { message: partial });
    let update = |assistant_message_event| AgentEvent::MessageUpdate {
        assistant_message_event,
    };
    emit(update(AssistantMessageEvent::TextStart {
        content_index: 0,
    }));
    emit(update(AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: text.clone(),
    }));
    emit(update(AssistantMessageEvent::TextEnd {
        content_index: 0,
        content: text,
    }));
    emit(AgentEvent::MessageEnd {
        message: answer.clone(),
    });
    context.messages.push(answer.clone());
    collected.push(answer);
}
