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

fn shell_frame(id: u32) -> Vec<u8> {
    let mut exec = Vec::new();
    exec.extend(var_field(1, u64::from(id)));
    exec.extend(msg(2, b"ls"));
    msg(2, &exec)
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
    wire.extend(frame(&shell_frame(9)));
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
                proto::decode(&frame.message),
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
