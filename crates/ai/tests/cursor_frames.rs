use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use yi_ai::cursor::proto::{self, frame};
use yi_ai::cursor::{Blobs, ToolFn, apply_message};

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
}

fn field(field: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(u64::from((field << 3) | 2), &mut out);
    varint(body.len() as u64, &mut out);
    out.extend_from_slice(body);
    out
}

fn str_field(num: u32, text: &str) -> Vec<u8> {
    field(num, text.as_bytes())
}

fn var_field(num: u32, value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    varint(u64::from(num << 3), &mut out);
    varint(value, &mut out);
    out
}

fn msg(num: u32, body: &[u8]) -> Vec<u8> {
    field(num, body)
}

fn text_delta(text: &str) -> Vec<u8> {
    msg(1, &msg(1, &str_field(1, text)))
}

fn tool_frame(id: u32, name: &str, call_id: &str) -> Vec<u8> {
    let mut entry = Vec::new();
    entry.extend(str_field(1, "path"));
    entry.extend(msg(2, &str_field(3, "a.rs")));
    let mut mcp = Vec::new();
    mcp.extend(str_field(1, name));
    mcp.extend(msg(2, &entry));
    mcp.extend(str_field(3, call_id));
    mcp.extend(str_field(5, name));
    let mut exec = Vec::new();
    exec.extend(var_field(1, u64::from(id)));
    exec.extend(msg(11, &mcp));
    msg(2, &exec)
}

fn exec_frame(id: u32, kind: u32, body: &[u8]) -> Vec<u8> {
    let mut exec = var_field(1, u64::from(id));
    exec.extend(msg(kind, body));
    msg(2, &exec)
}

fn shell_args(command: &str, cwd: &str) -> Vec<u8> {
    let mut args = str_field(1, command);
    args.extend(str_field(2, cwd));
    args
}

fn nested_arg(levels: usize) -> Vec<u8> {
    let mut value = str_field(3, "x");
    for _ in 1..levels {
        value = msg(6, &msg(1, &value));
    }
    let mut entry = str_field(1, "path");
    entry.extend(msg(2, &value));
    let mut mcp = str_field(1, "read");
    mcp.extend(msg(2, &entry));
    exec_frame(5, 11, &mcp)
}

fn is_throw(reply: &[u8]) -> bool {
    reply.first() == Some(&((5 << 3) | 2))
}

#[tokio::test]
async fn a_text_delta_a_tool_and_an_unknown_frame() -> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));
    let tool: ToolFn = Box::new({
        let calls = Arc::clone(&calls);
        move |_call_id, name, args| {
            calls.fetch_add(1, Ordering::SeqCst);
            let ok =
                name == "read" && args.get("path").and_then(|value| value.as_str()) == Some("a.rs");
            Box::pin(async move { if ok { "file body" } else { "wrong" }.to_owned() })
        }
    });
    let mut wire = frame(&text_delta("pong"));
    wire.extend(frame(&tool_frame(7, "read", "call-7")));
    wire.extend(frame(&exec_frame(9, 16, &str_field(1, "unmapped"))));
    let tail = frame(&text_delta("next"));
    wire.extend_from_slice(tail.get(..3).ok_or("short")?);
    let (ready, rest) = proto::split_ready(&wire);
    assert_eq!(ready.len(), 3);
    assert_eq!(rest.len(), 3);
    let mut blobs = Blobs::new();
    let mut applied = Vec::new();
    for frame in ready {
        applied.push(
            apply_message(
                proto::decode(&frame.message, "file:///"),
                &tool,
                &[],
                "file:///",
                &mut blobs,
            )
            .await,
        );
    }
    let [text, ran, unknown] = applied.as_slice() else {
        return Err("three frames".into());
    };
    assert!(text.events.iter().any(|event| matches!(
        event,
        yi_types::event::AssistantMessageEvent::TextDelta { delta, .. } if delta == "pong"
    )));
    assert!(text.replies.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(ran.replies.len(), 1);
    assert!(!is_throw(&ran.replies[0]));
    assert!(
        ran.replies[0]
            .windows(b"file body".len())
            .any(|window| window == b"file body")
    );
    assert_eq!(unknown.replies.len(), 2);
    assert!(is_throw(&unknown.replies[0]));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (whole, leftover) = proto::split_ready(&frame(&text_delta("pong")));
    assert_eq!((whole.len(), leftover.len()), (1, 0));
    Ok(())
}

#[tokio::test]
async fn a_shell_frame_runs_bash_and_answers_with_a_shell_result()
-> Result<(), Box<dyn std::error::Error>> {
    let tool: ToolFn = Box::new(|_call_id, name, args| {
        let ran = format!(
            "{name}: {}",
            args.get("command").and_then(|v| v.as_str()).unwrap_or("")
        );
        Box::pin(async move { ran })
    });
    let mut ran = Vec::new();
    for cwd in ["/work/lane", "/work"] {
        let inbound = proto::decode(&exec_frame(3, 2, &shell_args("ls", cwd)), "file:///work");
        let applied = apply_message(inbound, &tool, &[], "file:///work", &mut Blobs::new()).await;
        ran.push(applied.replies);
    }
    let [elsewhere, at_root] = ran.as_slice() else {
        return Err("two runs".into());
    };
    let shell_result = |cwd: &str, output: &str| {
        let mut result = shell_args("ls", cwd);
        result.extend(str_field(5, output));
        let mut exec = var_field(1, 3);
        exec.extend(msg(2, &msg(1, &result)));
        vec![msg(2, &exec), proto::stream_close(3)]
    };
    let wrapped = "bash: cd '/work/lane' && ls";
    assert_eq!(elsewhere, &shell_result("/work/lane", wrapped));
    assert_eq!(at_root, &shell_result("/work", "bash: ls"));
    Ok(())
}

#[test]
fn an_argument_nested_past_the_depth_limit_is_refused() {
    let at_limit = proto::decode(&nested_arg(64), "file:///");
    assert!(matches!(at_limit, Ok(proto::Inbound::Tool { .. })));
    let past_limit = proto::decode(&nested_arg(65), "file:///");
    assert!(matches!(past_limit, Ok(proto::Inbound::Reject { id: 5 })));
}

#[test]
fn the_prompt_labels_roles_and_names_what_cursor_never_receives()
-> Result<(), Box<dyn std::error::Error>> {
    let usage = serde_json::to_value(yi_types::message::Usage::zero())?;
    let context: yi_types::model::LlmContext = serde_json::from_value(serde_json::json!({
        "systemPrompt": "sys",
        "messages": [
            {"role": "user", "timestamp": 0, "content": [
                {"type": "text", "text": "what is in this?"},
                {"type": "image", "data": "AAAA", "mimeType": "image/png"}]},
            {"role": "assistant", "api": "a", "provider": "p", "model": "m", "usage": usage,
             "stopReason": "toolUse", "timestamp": 0, "content": [
                {"type": "toolCall", "id": "c1", "name": "read", "arguments": {}}]},
            {"role": "toolResult", "toolCallId": "c1", "toolName": "read", "isError": false,
             "timestamp": 0, "content": [{"type": "text", "text": "secret body"}]},
        ],
    }))?;
    let prompt = yi_ai::cursor::prompt_of(&context);
    let lines: Vec<&str> = prompt.lines().collect();
    assert_eq!(
        lines,
        [
            "sys",
            "User: what is in this?",
            "[… image dropped: the cursor provider receives text only]",
            "Assistant: [tool read]",
            "[… result of read dropped: the cursor provider does not receive tool results]",
        ]
    );
    Ok(())
}
