use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Map, Value};

// Incident: a server that logs over stdout would otherwise spin this loop
// forever waiting for an id that is never coming.
const MAX_SKIPPED_MESSAGES: u32 = 256;

/// MCP stdio framing: one JSON message per line, no embedded newlines.
pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    pub fn spawn(command: &str, args: &[String], env: &Map<String, Value>) -> Result<Self, String> {
        #[expect(
            clippy::disallowed_methods,
            reason = "an MCP stdio server is a child process by definition (design §7.6)"
        )]
        let mut builder = Command::new(command);
        builder.args(args);
        for (key, value) in env {
            if let Some(text) = value.as_str() {
                builder.env(key, text);
            }
        }
        let mut child = builder
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("spawn {command} failed: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "child stdin was not piped".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "child stdout was not piped".to_owned())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    pub fn send(&mut self, message: &Value) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|error| format!("write to server failed: {error}"))
    }

    pub fn round_trip(&mut self, message: &Value, id: u64) -> Result<Value, String> {
        self.send(message)?;
        let mut skipped: u32 = 0;
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(|error| format!("read from server failed: {error}"))?;
            if read == 0 {
                return Err("server closed the stream before replying".to_owned());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
                skipped = skipped.saturating_add(1);
                if skipped > MAX_SKIPPED_MESSAGES {
                    return Err("server sent no reply to the request".to_owned());
                }
                continue;
            };
            if parsed.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(parsed);
            }
            skipped = skipped.saturating_add(1);
            if skipped > MAX_SKIPPED_MESSAGES {
                return Err("server sent no reply to the request".to_owned());
            }
        }
    }

    pub fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn a_tool_argument_holding_a_newline_cannot_break_framing()
    -> Result<(), Box<dyn std::error::Error>> {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "write", "arguments": {"text": "one\ntwo\r\nthree"}},
        });
        let line = serde_json::to_string(&message)?;
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
        Ok(())
    }
}
