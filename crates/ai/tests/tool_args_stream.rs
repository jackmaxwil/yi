use serde_json::{Map, Value, json};
use std::error::Error;
use std::time::{Duration, Instant};
use yi_ai::json_salvage::parse_streaming_json;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content};

type TestResult = Result<(), Box<dyn Error>>;

const RECORDED_SSE: &str = include_str!("fixtures/kimi-chat-2026-10-24-bash.sse");
const WRITE_SOURCE: &str = include_str!("../../tui/src/app/stream.rs");

#[derive(Clone, Copy, Debug)]
enum Adapter {
    Anthropic,
    OpenAi,
    Responses,
}

const ADAPTERS: [Adapter; 3] = [Adapter::Anthropic, Adapter::OpenAi, Adapter::Responses];

enum Live {
    Anthropic(yi_ai::anthropic::Mapper),
    OpenAi(yi_ai::openai::ChunkMapper),
    Responses(yi_ai::openai_responses::EventMapper),
}

impl Live {
    fn open(adapter: Adapter) -> Self {
        let model = |api: &str| crate::common::model("m", api, "p", "http://h", None);
        let mut live = match adapter {
            Adapter::Anthropic => {
                Self::Anthropic(yi_ai::anthropic::Mapper::new(&model("anthropic-messages")))
            }
            Adapter::OpenAi => Self::OpenAi(yi_ai::openai::ChunkMapper::new(&model(
                "openai-completions",
            ))),
            Adapter::Responses => Self::Responses(yi_ai::openai_responses::EventMapper::new(
                &model("openai-responses"),
            )),
        };
        let _ = match &mut live {
            Self::Anthropic(mapper) => mapper.push(&json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"write","input":{}}})),
            Self::OpenAi(mapper) => mapper.push_chunk(&json!({"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"write","arguments":""}}]}}]})),
            Self::Responses(mapper) => mapper.push(&json!({"type":"response.output_item.added","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write","arguments":""}})),
        };
        live
    }

    fn delta(&mut self, chunk: &str) -> Vec<AssistantMessageEvent> {
        match self {
            Self::Anthropic(mapper) => mapper.push(&json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":chunk}})),
            Self::OpenAi(mapper) => mapper.push_chunk(&json!({"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":chunk}}]}}]})),
            Self::Responses(mapper) => mapper.push(&json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":chunk})),
        }
    }

    fn end(self) -> Vec<AssistantMessageEvent> {
        match self {
            Self::Anthropic(mut mapper) => {
                let mut events = mapper.push(&json!({"type":"content_block_stop","index":0}));
                events.extend(
                    mapper
                        .push(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
                );
                events.extend(mapper.push(&json!({"type":"message_stop"})));
                events.push(mapper.finish());
                events
            }
            Self::OpenAi(mut mapper) => {
                let mut events = mapper.push_chunk(
                    &json!({"id":"c1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                );
                events.extend(mapper.finish());
                events
            }
            Self::Responses(mut mapper) => {
                let mut events = mapper.push(&json!({"type":"response.function_call_arguments.done","item_id":"fc_1","call_id":"call_1","name":"write"}));
                events.extend(
                    mapper.push(&json!({"type":"response.completed","response":{"id":"r1"}})),
                );
                events.extend(mapper.finish());
                events
            }
        }
    }

    fn cut(self) -> Vec<AssistantMessageEvent> {
        match self {
            Self::Anthropic(mapper) => vec![mapper.finish()],
            Self::OpenAi(mapper) => mapper.finish(),
            Self::Responses(mut mapper) => mapper.finish(),
        }
    }
}

fn split(text: &str, size: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = size.min(rest.len());
        while !rest.is_char_boundary(end) {
            end = end.saturating_add(1);
        }
        let (piece, tail) = rest.split_at(end);
        pieces.push(piece);
        rest = tail;
    }
    pieces
}

fn ended_arguments(events: &[AssistantMessageEvent]) -> Option<&Map<String, Value>> {
    events.iter().find_map(|event| match event {
        AssistantMessageEvent::ToolCallEnd {
            tool_call: Content::ToolCall { arguments, .. },
            ..
        } => Some(arguments),
        _ => None,
    })
}

fn error_arguments(events: &[AssistantMessageEvent]) -> Option<&Map<String, Value>> {
    events.iter().find_map(|event| match event {
        AssistantMessageEvent::Error {
            error: AgentMessage::Assistant { content, .. },
            ..
        } => content.iter().find_map(|block| match block {
            Content::ToolCall { arguments, .. } => Some(arguments),
            _ => None,
        }),
        _ => None,
    })
}

fn recorded_arguments() -> Result<String, Box<dyn Error>> {
    let mut arguments = String::new();
    for line in RECORDED_SSE.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(piece) = chunk
            .pointer("/choices/0/delta/tool_calls/0/function/arguments")
            .and_then(Value::as_str)
        {
            arguments.push_str(piece);
        }
    }
    serde_json::from_str::<Value>(&arguments)?;
    Ok(arguments)
}

fn calls() -> Result<Vec<String>, Box<dyn Error>> {
    let write = json!({"path": "crates/tui/src/app/stream.rs", "content": WRITE_SOURCE.chars().take(160).collect::<String>()});
    Ok(vec![recorded_arguments()?, write.to_string()])
}

#[test]
fn a_two_hundred_kilobyte_write_streams_in_linear_time() -> TestResult {
    let mut content = String::new();
    while content.len() < 200_000 {
        content.push_str(WRITE_SOURCE);
    }
    let arguments = json!({"path": "crates/tui/src/app/stream.rs", "content": content}).to_string();
    let expected: Map<String, Value> = serde_json::from_str(&arguments)?;
    let bound = Duration::from_secs(10);
    for adapter in ADAPTERS {
        let started = Instant::now();
        let mut live = Live::open(adapter);
        for (index, piece) in split(&arguments, 16).into_iter().enumerate() {
            let _ = live.delta(piece);
            if started.elapsed() > bound {
                return Err(format!(
                    "{adapter:?}: {bound:?} spent by delta {index} of {} bytes",
                    arguments.len()
                )
                .into());
            }
        }
        let events = live.end();
        assert!(
            started.elapsed() < bound,
            "{adapter:?}: {:?}",
            started.elapsed()
        );
        assert_eq!(ended_arguments(&events), Some(&expected), "{adapter:?}");
    }
    Ok(())
}

#[test]
fn the_final_arguments_equal_the_one_shot_parse_at_every_split() -> TestResult {
    for call in calls()? {
        let expected: Map<String, Value> = serde_json::from_str(&call)?;
        for adapter in ADAPTERS {
            for size in 1..=call.len() {
                let mut live = Live::open(adapter);
                for piece in split(&call, size) {
                    let _ = live.delta(piece);
                }
                let events = live.end();
                assert_eq!(
                    ended_arguments(&events),
                    Some(&expected),
                    "{adapter:?} at chunk size {size}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn a_stream_cut_mid_call_keeps_the_best_effort_object() -> TestResult {
    for call in calls()? {
        for adapter in ADAPTERS {
            for cut in (0..=call.len()).filter(|cut| call.is_char_boundary(*cut)) {
                let prefix = call.get(..cut).ok_or("char boundary")?;
                let mut live = Live::open(adapter);
                if !prefix.is_empty() {
                    let _ = live.delta(prefix);
                }
                let events = live.cut();
                assert_eq!(
                    error_arguments(&events),
                    Some(&parse_streaming_json(prefix)),
                    "{adapter:?} cut at byte {cut} of {call}"
                );
            }
        }
    }
    Ok(())
}
