use serde_json::json;
use std::error::Error;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::model::{LlmContext, Reuse, ToolDef, Ttl};

const RECORDED: &str = include_str!("fixtures/build_params_2026-10-29.txt");

fn assistant(
    api: &str,
    stop: &str,
    content: serde_json::Value,
) -> Result<AgentMessage, Box<dyn Error>> {
    let zero = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,
        "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
    Ok(serde_json::from_value(
        json!({"role":"assistant","content":content,"api":api,
        "provider":if api == "m" { "p" } else { "x" },"model":if api == "m" { "m" } else { "y" },
        "usage":zero,"stopReason":stop,"timestamp":1}),
    )?)
}

fn history() -> Result<Vec<AgentMessage>, Box<dyn Error>> {
    let png = format!("iVBORw0KGgoAAAANSUhEUgAAAEAAAABA{}", "B".repeat(64));
    let image = |data: &str| Content::Image {
        data: data.to_owned(),
        mime_type: "image/png".to_owned(),
    };
    let text = |text: &str| Content::Text {
        text: text.to_owned(),
        text_signature: None,
    };
    Ok(vec![
        AgentMessage::user_input(
            UserContent::Blocks(vec![text("look"), image(&png), image("notbase64!!")]),
            0,
        ),
        assistant(
            "other",
            "toolUse",
            json!([
            {"type":"thinking","thinking":"hm","thinkingSignature":"sig"},
            {"type":"text","text":"ok"},
            {"type":"toolCall","id":"call|weird id","name":"bash","arguments":{"cmd":"ls"}},
            {"type":"toolCall","id":"orphan","name":"read","arguments":{"path":"a"}}]),
        )?,
        AgentMessage::ToolResult {
            tool_call_id: "call|weird id".to_owned(),
            tool_name: "bash".to_owned(),
            content: vec![text("out"), image(&png)],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 3,
        },
        assistant("m", "error", json!([{"type":"text","text":"bad"}]))?,
        AgentMessage::user_input(UserContent::Text("next".to_owned()), 4),
        AgentMessage::user_input(UserContent::Blocks(vec![image(&png)]), 5),
    ])
}

/// Bodies recorded from the adapters before history was borrowed: the wire may not move.
#[test]
fn every_adapter_builds_the_recorded_body() -> Result<(), Box<dyn Error>> {
    let tools = vec![ToolDef {
        name: "bash".to_owned(),
        description: "run".to_owned(),
        parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
        freeform: None,
    }];
    let mut recorded = RECORDED.lines();
    for vision in [true, false] {
        for base in ["http://h", "https://openrouter.ai/api/v1"] {
            for api in [
                "anthropic-messages",
                "openai-completions",
                "openai-responses",
            ] {
                let mut model = crate::common::model("m", api, "p", base, None);
                if vision {
                    model.input = vec!["text".to_owned(), "image".to_owned()];
                }
                for shared in [None, Some(2)] {
                    let context = LlmContext {
                        cache_ttl: Ttl::Min5,
                        system_prompt: "s\u{1d}t".to_owned(),
                        messages: history()?,
                        transient: vec![AgentMessage::user_input(
                            UserContent::Text("env".to_owned()),
                            9,
                        )],
                        schema: None,
                        shared_through: shared,
                        reuse: Reuse::Loop,
                        tools: Some(tools.clone()),
                        tool_choice: None,
                    };
                    let body = match api {
                        "anthropic-messages" => {
                            yi_ai::anthropic::build_params(&model, &context, &Default::default())
                        }
                        "openai-completions" => {
                            yi_ai::openai::build_params(&model, &context, &Default::default())
                        }
                        _ => yi_ai::openai_responses::build_params(
                            &model,
                            &context,
                            &Default::default(),
                        ),
                    };
                    let expected = recorded.next().ok_or("fixture ran out of bodies")?;
                    assert_eq!(
                        body.to_string(),
                        expected,
                        "{api} at {base}, vision {vision}, shared {shared:?}"
                    );
                }
            }
        }
    }
    assert!(
        recorded.next().is_none(),
        "fixture holds more bodies than built"
    );
    Ok(())
}
