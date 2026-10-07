use std::sync::atomic::{AtomicUsize, Ordering};

use yi_ai::cursor::apply_message;
use yi_ai::cursor::proto::{self, frame};

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
    let mut mcp = Vec::new();
    mcp.extend(str_field(1, name));
    mcp.extend(str_field(2, call_id));
    mcp.extend(str_field(3, r#"{"path":"a.rs"}"#));
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

#[test]
fn a_text_delta_a_tool_and_an_unknown_frame() {
    let calls = AtomicUsize::new(0);
    let mut tool = |name: &str, args: &serde_json::Map<String, serde_json::Value>| {
        calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(name, "read");
        assert_eq!(args.get("path").and_then(|value| value.as_str()), Some("a.rs"));
        "file body".to_owned()
    };
    let text = apply_message(&text_delta("pong"), &mut tool);
    assert!(text.events.iter().any(|event| matches!(
        event,
        yi_types::event::AssistantMessageEvent::TextDelta { delta, .. } if delta == "pong"
    )));
    assert!(text.replies.is_empty());
    let tool_applied = apply_message(&tool_frame(7, "read", "call-7"), &mut tool);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool_applied.replies.len(), 1);
    assert!(!proto::is_throw(&tool_applied.replies[0]));
    let unknown = apply_message(&shell_frame(9), &mut tool);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(unknown.replies.len(), 1);
    assert!(proto::is_throw(&unknown.replies[0]));
    assert_eq!(proto::frames(&frame(&text_delta("pong"))).expect("frames").len(), 1);
}
