use serde_json::{Map, Value};

pub const CLIENT_VERSION: &str = "cli-2026.07.23-e383d2b";
pub const RUN_PATH: &str = "/agent.v1.AgentService/Run";
pub const NAMESPACE: &str = "cursor";

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

fn key(field: u32, wire: u8, out: &mut Vec<u8>) {
    varint(u64::from((field << 3) | u32::from(wire)), out);
}

fn field_varint(field: u32, n: u64, out: &mut Vec<u8>) {
    key(field, 0, out);
    varint(n, out);
}

fn field_bytes(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    key(field, 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn field_str(field: u32, text: &str, out: &mut Vec<u8>) {
    field_bytes(field, text.as_bytes(), out);
}

fn field_msg(field: u32, body: &[u8], out: &mut Vec<u8>) {
    field_bytes(field, body, out);
}

pub fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + message.len());
    out.push(0);
    let len = u32::try_from(message.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(message);
    out
}

pub fn frames(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest.len() < 5 {
            return Err("short connect frame".to_owned());
        }
        let flag = rest[0];
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if rest.len() < 5 + len {
            return Err("truncated connect frame".to_owned());
        }
        out.push(Frame {
            end: flag & 0x02 != 0,
            message: rest[5..5 + len].to_vec(),
        });
        rest = &rest[5 + len..];
    }
    Ok(out)
}

pub struct Frame {
    pub end: bool,
    pub message: Vec<u8>,
}

struct Raw {
    field: u32,
    bytes: Vec<u8>,
    varint: Option<u64>,
}

fn walk(buf: &[u8]) -> Result<Vec<Raw>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let (tag, next) = take_varint(buf, i)?;
        i = next;
        let field = u32::try_from(tag >> 3).unwrap_or(0);
        let wire = (tag & 7) as u8;
        match wire {
            0 => {
                let (value, next) = take_varint(buf, i)?;
                i = next;
                out.push(Raw {
                    field,
                    bytes: Vec::new(),
                    varint: Some(value),
                });
            }
            2 => {
                let (len, next) = take_varint(buf, i)?;
                i = next;
                let len = usize::try_from(len).unwrap_or(0);
                let end = i.saturating_add(len);
                if end > buf.len() {
                    return Err("truncated protobuf field".to_owned());
                }
                out.push(Raw {
                    field,
                    bytes: buf[i..end].to_vec(),
                    varint: None,
                });
                i = end;
            }
            1 => i = i.saturating_add(8),
            5 => i = i.saturating_add(4),
            _ => return Err(format!("unsupported protobuf wire {wire}")),
        }
    }
    Ok(out)
}

fn take_varint(buf: &[u8], mut i: usize) -> Result<(u64, usize), String> {
    let mut n = 0u64;
    let mut shift = 0;
    while i < buf.len() && shift < 64 {
        let byte = buf[i];
        i += 1;
        n |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((n, i));
        }
        shift += 7;
    }
    Err("bad varint".to_owned())
}

fn string_of(raw: &Raw) -> String {
    String::from_utf8_lossy(&raw.bytes).into_owned()
}

pub fn run_request(
    prompt: &str,
    model_id: &str,
    conversation_id: &str,
    tools: &[ToolWire],
) -> Vec<u8> {
    let mut state = Vec::new();
    field_varint(10, 1, &mut state);
    field_str(22, "yi", &mut state);
    let mut user = Vec::new();
    field_str(1, prompt, &mut user);
    field_str(2, conversation_id, &mut user);
    field_varint(4, 1, &mut user);
    let mut action = Vec::new();
    field_msg(1, &user, &mut action);
    let mut model = Vec::new();
    field_str(1, model_id, &mut model);
    let mut run = Vec::new();
    field_msg(1, &state, &mut run);
    field_msg(2, &action, &mut run);
    for tool in tools {
        let mut def = Vec::new();
        field_str(1, &tool.name, &mut def);
        field_str(2, &tool.description, &mut def);
        let mut value = Vec::new();
        field_str(3, &tool.schema, &mut value);
        field_msg(3, &value, &mut def);
        field_str(4, "yi", &mut def);
        field_msg(4, &def, &mut run);
    }
    field_str(5, conversation_id, &mut run);
    field_msg(9, &model, &mut run);
    field_varint(19, 1, &mut run);
    let mut client = Vec::new();
    field_msg(1, &run, &mut client);
    client
}

pub struct ToolWire {
    pub name: String,
    pub description: String,
    pub schema: String,
}

pub enum Inbound {
    Text(String),
    Thinking(String),
    TurnEnded,
    Tool {
        id: u32,
        name: String,
        call_id: String,
        args: Map<String, Value>,
    },
    Reject {
        id: u32,
    },
    Ignore,
}

pub fn decode(message: &[u8]) -> Result<Inbound, String> {
    let outer = walk(message)?;
    if outer.iter().any(|raw| raw.field > 8) {
        return Err("unknown agent server field".to_owned());
    }
    if let Some(update) = outer
        .iter()
        .find(|raw| raw.field == 1 && raw.varint.is_none())
    {
        return decode_update(&update.bytes);
    }
    if let Some(exec) = outer
        .iter()
        .find(|raw| raw.field == 2 && raw.varint.is_none())
    {
        return decode_exec(&exec.bytes);
    }
    Ok(Inbound::Ignore)
}

fn decode_update(bytes: &[u8]) -> Result<Inbound, String> {
    let fields = walk(bytes)?;
    if let Some(text) = fields
        .iter()
        .find(|raw| raw.field == 1 && raw.varint.is_none())
    {
        let inner = walk(&text.bytes)?;
        let value = inner
            .iter()
            .find(|raw| raw.field == 1)
            .map(string_of)
            .unwrap_or_default();
        return Ok(Inbound::Text(value));
    }
    if let Some(thinking) = fields
        .iter()
        .find(|raw| raw.field == 4 && raw.varint.is_none())
    {
        let inner = walk(&thinking.bytes)?;
        let value = inner
            .iter()
            .find(|raw| raw.field == 1)
            .map(string_of)
            .unwrap_or_default();
        return Ok(Inbound::Thinking(value));
    }
    if fields.iter().any(|raw| raw.field == 14) {
        return Ok(Inbound::TurnEnded);
    }
    Ok(Inbound::Ignore)
}

fn decode_exec(bytes: &[u8]) -> Result<Inbound, String> {
    let fields = walk(bytes)?;
    let id = fields
        .iter()
        .find(|raw| raw.field == 1)
        .and_then(|raw| raw.varint)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    if let Some(mcp) = fields
        .iter()
        .find(|raw| raw.field == 11 && raw.varint.is_none())
    {
        let inner = walk(&mcp.bytes)?;
        let name = inner
            .iter()
            .find(|raw| raw.field == 1)
            .map(string_of)
            .unwrap_or_default();
        let call_id = inner
            .iter()
            .find(|raw| raw.field == 2)
            .map(string_of)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("cursor-{id}"));
        let args_text = inner
            .iter()
            .find(|raw| raw.field == 3)
            .map(string_of)
            .unwrap_or_else(|| "{}".to_owned());
        let args = serde_json::from_str(&args_text).unwrap_or_default();
        return Ok(Inbound::Tool {
            id,
            name,
            call_id,
            args,
        });
    }
    let rejected = [2, 3, 4, 5, 7, 8, 9];
    if fields.iter().any(|raw| rejected.contains(&raw.field)) {
        return Ok(Inbound::Reject { id });
    }
    Ok(Inbound::Ignore)
}

pub fn throw_reply(id: u32, error: &str) -> Vec<u8> {
    let mut thrown = Vec::new();
    field_varint(1, u64::from(id), &mut thrown);
    field_str(2, error, &mut thrown);
    let mut control = Vec::new();
    field_msg(2, &thrown, &mut control);
    let mut client = Vec::new();
    field_msg(5, &control, &mut client);
    client
}

pub fn tool_reply(id: u32, output: &str) -> Vec<u8> {
    let mut success = Vec::new();
    field_str(1, output, &mut success);
    let mut exec = Vec::new();
    field_varint(1, u64::from(id), &mut exec);
    field_msg(11, &success, &mut exec);
    let mut client = Vec::new();
    field_msg(2, &exec, &mut client);
    client
}

pub fn is_throw(message: &[u8]) -> bool {
    walk(message)
        .ok()
        .is_some_and(|fields| fields.iter().any(|raw| raw.field == 5))
}
