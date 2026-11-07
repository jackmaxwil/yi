use tokio::sync::mpsc::Sender;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::Model;

use super::ChunkMapper;
use crate::breakpoints::Encoded;
use crate::json_salvage::parse_json_with_repair;

/// A turn whose stream never reached its usage chunk, settled from the generation record
/// (D163); with a hook, the fetch is returned as a job to start once the turn has ended.
fn settle_from_record(
    output: &mut AgentMessage,
    model: &Model,
    wire: &crate::request::Wire<'_>,
) -> Option<crate::settle::Job> {
    let AgentMessage::Assistant {
        usage,
        response_id: Some(id),
        ..
    } = output
    else {
        return None;
    };
    if !usage.unknown || !crate::settle::settles(model) {
        return None;
    }
    if let Some(hook) = wire.settle {
        let (hook, model, id) = (hook.clone(), model.clone(), id.clone());
        let (key, proxy) = (wire.api_key.to_owned(), wire.proxy.cloned());
        return Some(crate::settle::job(move || {
            if let Some((settled, upstream)) =
                crate::settle::generation_usage(&model, &key, proxy.as_ref(), &id)
            {
                (hook.0)(settled, upstream);
            }
        }));
    }
    if let Some((settled, upstream)) =
        crate::settle::generation_usage(model, wire.api_key, wire.proxy, id)
    {
        *usage = settled;
        if let Some(upstream) = upstream {
            crate::request::note_upstream(output, &upstream);
        }
    }
    None
}

fn detach_settlement(job: Option<crate::settle::Job>) {
    if let Some(job) = job {
        crate::settle::detach(job);
    }
}

pub(super) fn run_request(
    model: &Model,
    body: &Encoded,
    wire: crate::request::Wire<'_>,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let url = format!("{}/chat/completions", model.base_url);
    let mut mapper = ChunkMapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    let retried = crate::request::waiting(sender);
    let stripped = crate::request::Stripped::default();
    let retry = crate::request::Retry::new(model.read_timeout(), wire.stop, &retried);
    let pumped = crate::request::pump_sse_with_resend(
        wire.stop,
        &retry,
        || crate::request::openai_bearer_post(&url, model, body, wire, &retry, &stripped),
        |sse| {
            if sse.data == "[DONE]" {
                return Ok(true);
            }
            let data = sse.data.trim();
            if data.is_empty() {
                return Ok(true);
            }
            // Incident: an unparsable line was dropped, so a body that stopped being SSE read as
            // an empty turn; Anthropic has failed its turn on one since the mapper existed.
            let payload = parse_json_with_repair(data).map_err(|error| {
                format!(
                    "Could not parse chat SSE event: {error}: {}",
                    data.chars().take(200).collect::<String>()
                )
            })?;
            for event in mapper.push_chunk(&payload) {
                let _ = sender.blocking_send(event);
            }
            Ok(true)
        },
    );
    crate::request::record_stripped(&mut mapper.output, &stripped);
    let resent = match pumped {
        Ok(resent) => resent,
        // a stream that died after its first chunk was billed for what it generated: the
        // error turn is settled from the record (D79, #331); one that died before any chunk
        // has no record and stays unknown
        Err(message) => {
            detach_settlement(settle_from_record(&mut mapper.output, model, &wire));
            let _ = sender.blocking_send(mapper.fail(&crate::request::auth_hint(
                &message,
                wire.oauth,
                &model.provider,
            )));
            return Ok(());
        }
    };
    crate::request::note_reply(&mut mapper.output, resent, &stripped);
    // a cut dropped the stream before its finish and usage chunks: the turn ends as a length
    // stop and is settled from the generation record (D163)
    let cut = wire
        .stop
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
    if cut {
        mapper.cut();
    }
    // an in-band error chunk ends the stream before its usage chunk: settled the same way (D175)
    let failed = matches!(
        mapper.output,
        AgentMessage::Assistant {
            stop_reason: StopReason::Error,
            ..
        }
    );
    detach_settlement(
        (cut || failed)
            .then(|| settle_from_record(&mut mapper.output, model, &wire))
            .flatten(),
    );
    for event in mapper.finish() {
        let _ = sender.blocking_send(event);
    }
    Ok(())
}
