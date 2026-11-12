use serde_json::json;
use std::error::Error;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::model::{LlmContext, Model, Reuse, ToolDef, Ttl};

const RECORDED: &str = include_str!("fixtures/build_params_2026-11-09.txt");

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

fn wire_body(api: &str, model: &Model, context: &LlmContext) -> String {
    match api {
        "cursor-agent" => yi_ai::cursor::prompt_of(context),
        "anthropic-messages" => {
            yi_ai::anthropic::build_params(model, context, &Default::default()).to_string()
        }
        "openai-completions" => {
            yi_ai::openai::build_params(model, context, &Default::default()).to_string()
        }
        _ => yi_ai::openai_responses::build_params(model, context, &Default::default()).to_string(),
    }
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
                        cache_tail: true,
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
                    let body = wire_body(api, &model, &context);
                    let expected = recorded.next().ok_or("fixture ran out of bodies")?;
                    assert_eq!(
                        body, expected,
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

/// Dies with a block a newer build wrote sent to a provider that cannot read it, or dropped with
/// nothing in its place: every wire carries one notice per block (Cursor's drops tool results).
#[test]
fn a_block_this_build_cannot_read_reaches_every_wire_as_its_notice() -> Result<(), Box<dyn Error>> {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../types/tests/fixtures/v4-unknown-content.jsonl"
    );
    let mut messages = Vec::new();
    for line in std::fs::read_to_string(fixture)?.lines().skip(1) {
        if let yi_types::wire::Mutation::Entry {
            entry: yi_types::entry::Entry::Message { message, .. },
            ..
        } = serde_json::from_str(line)?
        {
            messages.push(message);
        }
    }
    let context = LlmContext {
        cache_ttl: Ttl::Min5,
        cache_tail: false,
        system_prompt: "s".to_owned(),
        messages,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: Reuse::Loop,
        tools: None,
        tool_choice: None,
    };
    for (api, vision) in [
        "anthropic-messages",
        "openai-completions",
        "openai-responses",
        "cursor-agent",
    ]
    .into_iter()
    .flat_map(|api| [(api, true), (api, false)])
    {
        let mut model = crate::common::model("m", api, "p", "http://h", None);
        if vision {
            model.input = vec!["text".to_owned(), "image".to_owned()];
        }
        let body = wire_body(api, &model, &context);
        let blocks = if api == "cursor-agent" { 3 } else { 4 };
        let notices = body.matches(yi_types::message::UNREAD_CONTENT).count();
        assert_eq!(notices, blocks, "{api}, vision {vision}: {body}");
        for raw in [
            "webSearchToolResult",
            "serverToolUse",
            "UklGRg==",
            "file:///tmp",
        ] {
            assert!(
                !body.contains(raw),
                "{api}, vision {vision}, sent {raw}: {body}"
            );
        }
    }
    Ok(())
}

/// Dies with a 400 on every later request: a failed reply is left out with its call, and the
/// call's result must reach no wire alone, nor its text in any role; one earlier call still answers.
#[test]
fn a_result_whose_reply_failed_reaches_every_wire_as_a_notice() -> Result<(), Box<dyn Error>> {
    let result = |id: &str, text: &str| AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: "bash".to_owned(),
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 2,
    };
    let call =
        |id: &str| json!([{"type":"toolCall","id":id,"name":"bash","arguments":{"cmd":"ls"}}]);
    let context = LlmContext {
        cache_ttl: Ttl::Min5,
        cache_tail: false,
        system_prompt: "s".to_owned(),
        messages: vec![
            AgentMessage::user_input(UserContent::Text("look".to_owned()), 0),
            assistant("m", "toolUse", call("kept_1"))?,
            AgentMessage::user_input(UserContent::Text("a note".to_owned()), 1),
            result("kept_1", "kept output"),
            assistant("m", "error", call("dropped_1"))?,
            result("dropped_1", "fetched page text"),
            AgentMessage::user_input(UserContent::Text("go on".to_owned()), 3),
        ],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: Reuse::Loop,
        tools: None,
        tool_choice: None,
    };
    let notice = "[bash result omitted: the call it answers is not in this request; run the tool again for its output]";
    for api in [
        "anthropic-messages",
        "openai-completions",
        "openai-responses",
    ] {
        let model = crate::common::model("m", api, "p", "http://h", None);
        let body = wire_body(api, &model, &context);
        assert!(
            !body.contains("fetched page text"),
            "{api} sent the output: {body}"
        );
        assert!(
            !body.contains("dropped_1"),
            "{api} sent the dropped call or its result: {body}"
        );
        assert_eq!(body.matches(notice).count(), 1, "{api}: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body)?;
        let items = (parsed.get("messages").or_else(|| parsed.get("input")))
            .and_then(serde_json::Value::as_array)
            .ok_or("no message list")?;
        let holder = (items.iter())
            .find(|item| item.to_string().contains(notice))
            .ok_or("no notice")?;
        assert_eq!(holder["role"], "user", "{api}: {body}");
    }
    Ok(())
}
