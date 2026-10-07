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
            1 => {
                let end = i.saturating_add(8);
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

fn value_from(bytes: &[u8]) -> Value {
    let Ok(fields) = walk(bytes) else {
        return Value::Null;
    };
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 3 && raw.varint.is_none())
    {
        return Value::String(string_of(raw));
    }
    if let Some(raw) = fields.iter().find(|raw| raw.field == 4) {
        return Value::Bool(raw.varint.unwrap_or(0) != 0);
    }
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 2 && raw.bytes.len() == 8)
    {
        let mut bits = [0u8; 8];
        bits.copy_from_slice(&raw.bytes);
        return serde_json::Number::from_f64(f64::from_le_bytes(bits))
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 6 && raw.varint.is_none())
    {
        let Ok(items) = walk(&raw.bytes) else {
            return Value::Null;
        };
        return Value::Array(
            items
                .iter()
                .filter(|raw| raw.field == 1 && raw.varint.is_none())
                .map(|raw| value_from(&raw.bytes))
                .collect(),
        );
    }
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 5 && raw.varint.is_none())
    {
        let Ok(entries) = walk(&raw.bytes) else {
            return Value::Null;
        };
        let mut map = Map::new();
        for entry in entries
            .iter()
            .filter(|raw| raw.field == 1 && raw.varint.is_none())
        {
            let Ok(pair) = walk(&entry.bytes) else {
                continue;
            };
            let key = pair
                .iter()
                .find(|raw| raw.field == 1)
                .map(string_of)
                .unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            let value = pair
                .iter()
                .find(|raw| raw.field == 2 && raw.varint.is_none())
                .map(|raw| value_from(&raw.bytes))
                .unwrap_or(Value::Null);
            map.insert(key, value);
        }
        return Value::Object(map);
    }
    Value::Null
}

pub fn run_request(
    prompt: &str,
    model_id: &str,
    conversation_id: &str,
    tools: &[ToolWire],
    workspace: &str,
) -> Vec<u8> {
    let mut state = Vec::new();
    field_str(9, workspace, &mut state);
    field_varint(10, 1, &mut state);
    field_str(22, "yi", &mut state);
    let mut user = Vec::new();
    field_str(1, prompt, &mut user);
    field_str(2, conversation_id, &mut user);
    field_varint(4, 1, &mut user);
    let mut message_action = Vec::new();
    field_msg(1, &user, &mut message_action);
    let mut env = Vec::new();
    field_str(2, workspace, &mut env);
    let mut context = Vec::new();
    field_msg(4, &env, &mut context);
    field_msg(2, &context, &mut message_action);
    let mut action = Vec::new();
    field_msg(1, &message_action, &mut action);
    let mut model = Vec::new();
    field_str(1, model_id, &mut model);
    let mut run = Vec::new();
    field_msg(1, &state, &mut run);
    field_msg(2, &action, &mut run);
    let mut listed = Vec::new();
    for tool in tools {
        field_msg(1, &tool_def(tool), &mut listed);
    }
    field_msg(4, &listed, &mut run);
    field_str(5, conversation_id, &mut run);
    field_msg(9, &model, &mut run);
    field_varint(19, 1, &mut run);
    let mut client = Vec::new();
    field_msg(1, &run, &mut client);
    client
}

#[derive(Clone)]
pub struct ToolWire {
    pub name: String,
    pub description: String,
    pub schema: String,
}

fn tool_def(tool: &ToolWire) -> Vec<u8> {
    let mut def = Vec::new();
    field_str(1, &tool.name, &mut def);
    field_str(2, &tool.description, &mut def);
    let mut schema = Vec::new();
    let json = serde_json::from_str(&tool.schema).unwrap_or(Value::Object(Map::new()));
    value_of(&json, &mut schema);
    field_msg(3, &schema, &mut def);
    field_str(4, "yi", &mut def);
    field_str(5, &tool.name, &mut def);
    def
}

fn value_of(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => field_varint(1, 0, out),
        Value::Bool(flag) => field_varint(4, u64::from(*flag), out),
        Value::Number(number) => {
            let bits = number.as_f64().unwrap_or(0.0).to_bits();
            key(2, 1, out);
            out.extend_from_slice(&bits.to_le_bytes());
        }
        Value::String(text) => field_str(3, text, out),
        Value::Array(items) => {
            let mut list = Vec::new();
            for item in items {
                let mut one = Vec::new();
                value_of(item, &mut one);
                field_msg(1, &one, &mut list);
            }
            field_msg(6, &list, out);
        }
        Value::Object(map) => {
            let mut strukt = Vec::new();
            for (name, item) in map {
                let mut entry = Vec::new();
                field_str(1, name, &mut entry);
                let mut nested = Vec::new();
                value_of(item, &mut nested);
                field_msg(2, &nested, &mut entry);
                field_msg(1, &entry, &mut strukt);
            }
            field_msg(5, &strukt, out);
        }
    }
}

pub enum Route {
    Mcp,
    Shell,
    ShellStream,
}

pub enum Inbound {
    Text(String),
    Thinking(String),
    TurnEnded,
    Tool {
        id: u32,
        name: String,
        call_id: String,
        exec_id: String,
        args: Map<String, Value>,
        route: Route,
        command: String,
        cwd: String,
    },
    Context {
        id: u32,
        exec_id: String,
    },
    KvGet {
        id: u32,
        blob_id: Vec<u8>,
    },
    KvSet {
        id: u32,
        blob_id: Vec<u8>,
        data: Vec<u8>,
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
    if let Some(kv) = outer
        .iter()
        .find(|raw| raw.field == 4 && raw.varint.is_none())
    {
        return decode_kv(&kv.bytes);
    }
    Ok(Inbound::Ignore)
}

fn decode_kv(bytes: &[u8]) -> Result<Inbound, String> {
    let fields = walk(bytes)?;
    let id = fields
        .iter()
        .find(|raw| raw.field == 1)
        .and_then(|raw| raw.varint)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    if let Some(get) = fields
        .iter()
        .find(|raw| raw.field == 2 && raw.varint.is_none())
    {
        let inner = walk(&get.bytes)?;
        let blob_id = inner
            .iter()
            .find(|raw| raw.field == 1)
            .map(|raw| raw.bytes.clone())
            .unwrap_or_default();
        return Ok(Inbound::KvGet { id, blob_id });
    }
    if let Some(set) = fields
        .iter()
        .find(|raw| raw.field == 3 && raw.varint.is_none())
    {
        let inner = walk(&set.bytes)?;
        let blob_id = inner
            .iter()
            .find(|raw| raw.field == 1)
            .map(|raw| raw.bytes.clone())
            .unwrap_or_default();
        let data = inner
            .iter()
            .find(|raw| raw.field == 2)
            .map(|raw| raw.bytes.clone())
            .unwrap_or_default();
        return Ok(Inbound::KvSet { id, blob_id, data });
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

fn shell_tool(id: u32, exec_id: String, bytes: &[u8], route: Route) -> Result<Inbound, String> {
    let Ok(inner) = walk(bytes) else {
        return Ok(Inbound::Reject { id });
    };
    let command = inner
        .iter()
        .find(|raw| raw.field == 1 && raw.varint.is_none())
        .map(string_of)
        .unwrap_or_default();
    if command.is_empty() {
        return Ok(Inbound::Reject { id });
    }
    let cwd = inner
        .iter()
        .find(|raw| raw.field == 2 && raw.varint.is_none())
        .map(string_of)
        .unwrap_or_default();
    let call_id = inner
        .iter()
        .find(|raw| raw.field == 4 && raw.varint.is_none())
        .map(string_of)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("cursor-{id}"));
    let run = if cwd.is_empty() {
        command.clone()
    } else {
        format!("cd -- '{}' && {command}", cwd.replace('\'', "'\\''"))
    };
    let mut args = Map::new();
    args.insert("command".to_owned(), Value::String(run));
    Ok(Inbound::Tool {
        id,
        name: "bash".to_owned(),
        call_id,
        exec_id,
        args,
        route,
        command,
        cwd,
    })
}

fn decode_exec(bytes: &[u8]) -> Result<Inbound, String> {
    let fields = walk(bytes)?;
    let id = fields
        .iter()
        .find(|raw| raw.field == 1)
        .and_then(|raw| raw.varint)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    let exec_id = fields
        .iter()
        .find(|raw| raw.field == 15 && raw.varint.is_none())
        .map(string_of)
        .unwrap_or_default();
    if fields
        .iter()
        .any(|raw| raw.field == 10 && raw.varint.is_none())
    {
        return Ok(Inbound::Context { id, exec_id });
    }
    if let Some(mcp) = fields
        .iter()
        .find(|raw| raw.field == 11 && raw.varint.is_none())
    {
        let inner = walk(&mcp.bytes)?;
        let name = inner
            .iter()
            .find(|raw| raw.field == 5)
            .map(string_of)
            .filter(|text| !text.is_empty())
            .or_else(|| {
                inner
                    .iter()
                    .find(|raw| raw.field == 1)
                    .map(string_of)
                    .filter(|text| !text.is_empty())
            })
            .unwrap_or_default();
        let name = name.strip_prefix("mcp_yi_").unwrap_or(&name).to_owned();
        let call_id = inner
            .iter()
            .find(|raw| raw.field == 3)
            .map(string_of)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("cursor-{id}"));
        let mut args = Map::new();
        for raw in inner
            .iter()
            .filter(|raw| raw.field == 2 && raw.varint.is_none())
        {
            let entry = walk(&raw.bytes)?;
            let key = entry
                .iter()
                .find(|raw| raw.field == 1)
                .map(string_of)
                .unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            let value = entry
                .iter()
                .find(|raw| raw.field == 2 && raw.varint.is_none())
                .map(|raw| value_from(&raw.bytes))
                .unwrap_or(Value::Null);
            args.insert(key, value);
        }
        return Ok(Inbound::Tool {
            id,
            name,
            call_id,
            exec_id,
            args,
            route: Route::Mcp,
            command: String::new(),
            cwd: String::new(),
        });
    }
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 2 && raw.varint.is_none())
    {
        return shell_tool(id, exec_id, &raw.bytes, Route::Shell);
    }
    if let Some(raw) = fields
        .iter()
        .find(|raw| raw.field == 14 && raw.varint.is_none())
    {
        return shell_tool(id, exec_id, &raw.bytes, Route::ShellStream);
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

pub fn tool_frames(
    route: Route,
    id: u32,
    exec_id: &str,
    command: &str,
    cwd: &str,
    output: &str,
) -> Vec<Vec<u8>> {
    match route {
        Route::Mcp => vec![tool_reply(id, exec_id, output)],
        Route::Shell => shell_replies(id, exec_id, command, cwd, output, false),
        Route::ShellStream => shell_replies(id, exec_id, command, cwd, output, true),
    }
}

pub fn tool_reply(id: u32, exec_id: &str, output: &str) -> Vec<u8> {
    let mut text = Vec::new();
    field_str(1, output, &mut text);
    let mut item = Vec::new();
    field_msg(1, &text, &mut item);
    let mut success = Vec::new();
    field_msg(1, &item, &mut success);
    let mut wrapped = Vec::new();
    field_msg(1, &success, &mut wrapped);
    let mut exec = Vec::new();
    field_varint(1, u64::from(id), &mut exec);
    if !exec_id.is_empty() {
        field_str(15, exec_id, &mut exec);
    }
    field_msg(11, &wrapped, &mut exec);
    let mut client = Vec::new();
    field_msg(2, &exec, &mut client);
    client
}

pub fn shell_replies(
    id: u32,
    exec_id: &str,
    command: &str,
    cwd: &str,
    output: &str,
    stream: bool,
) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    if stream {
        if !output.is_empty() {
            let mut data = Vec::new();
            field_str(1, output, &mut data);
            let mut event = Vec::new();
            field_msg(1, &data, &mut event);
            frames.push(exec_result(id, exec_id, 14, &event));
        }
        let mut exit = Vec::new();
        field_str(2, cwd, &mut exit);
        let mut done = Vec::new();
        field_msg(3, &exit, &mut done);
        frames.push(exec_result(id, exec_id, 14, &done));
    } else {
        let mut inner = Vec::new();
        field_str(1, command, &mut inner);
        field_str(2, cwd, &mut inner);
        field_str(5, output, &mut inner);
        let mut success = Vec::new();
        field_msg(1, &inner, &mut success);
        frames.push(exec_result(id, exec_id, 2, &success));
    }
    frames.push(stream_close(id));
    frames
}

fn exec_result(id: u32, exec_id: &str, field: u32, body: &[u8]) -> Vec<u8> {
    let mut exec = Vec::new();
    field_varint(1, u64::from(id), &mut exec);
    if !exec_id.is_empty() {
        field_str(15, exec_id, &mut exec);
    }
    field_msg(field, body, &mut exec);
    let mut client = Vec::new();
    field_msg(2, &exec, &mut client);
    client
}

pub fn stream_close(id: u32) -> Vec<u8> {
    let mut close = Vec::new();
    field_varint(1, u64::from(id), &mut close);
    let mut control = Vec::new();
    field_msg(1, &close, &mut control);
    let mut client = Vec::new();
    field_msg(5, &control, &mut client);
    client
}

pub fn context_reply(id: u32, exec_id: &str, workspace: &str, tools: &[ToolWire]) -> Vec<u8> {
    let mut env = Vec::new();
    field_str(2, workspace, &mut env);
    let mut ctx = Vec::new();
    field_msg(4, &env, &mut ctx);
    for tool in tools {
        field_msg(7, &tool_def(tool), &mut ctx);
    }
    let mut success = Vec::new();
    field_msg(1, &ctx, &mut success);
    let mut exec = Vec::new();
    field_varint(1, u64::from(id), &mut exec);
    if !exec_id.is_empty() {
        field_str(15, exec_id, &mut exec);
    }
    field_msg(10, &success, &mut exec);
    let mut client = Vec::new();
    field_msg(2, &exec, &mut client);
    client
}

pub fn kv_get_reply(id: u32, data: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    field_bytes(1, data, &mut got);
    let mut kv = Vec::new();
    field_varint(1, u64::from(id), &mut kv);
    field_msg(2, &got, &mut kv);
    let mut client = Vec::new();
    field_msg(3, &kv, &mut client);
    client
}

pub fn kv_set_reply(id: u32) -> Vec<u8> {
    let mut kv = Vec::new();
    field_varint(1, u64::from(id), &mut kv);
    field_msg(3, &[], &mut kv);
    let mut client = Vec::new();
    field_msg(3, &kv, &mut client);
    client
}

pub fn heartbeat() -> Vec<u8> {
    let mut client = Vec::new();
    field_msg(7, &[], &mut client);
    client
}

pub fn is_throw(message: &[u8]) -> bool {
    walk(message)
        .ok()
        .is_some_and(|fields| fields.iter().any(|raw| raw.field == 5))
}
