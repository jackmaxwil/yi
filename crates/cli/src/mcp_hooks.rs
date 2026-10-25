use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_runtime::claude_hooks::{Call, Event, Hooks, Outcome, Verdict};

/// Claude Code hooks around a cell's MCP call, which never passes the tool adapter; one-shot,
/// so the configuration loads per call, from this process's cwd.
pub struct McpHooks {
    hooks: Arc<Hooks>,
    cwd: std::path::PathBuf,
    rows: Mutex<Vec<String>>,
}

impl McpHooks {
    pub fn load() -> Option<Self> {
        let cwd = std::env::current_dir().ok()?;
        let hooks = Hooks::load(&cwd)?;
        let rows = Mutex::new(hooks.notices().to_vec());
        Some(Self { hooks, cwd, rows })
    }

    fn run(&self, event: Event, (server, tool, arguments): Named<'_>, call: Call) -> Outcome {
        let never: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| false);
        let call = Call {
            tool: format!("mcp__{server}__{tool}"),
            args: arguments.clone(),
            cwd: self.cwd.clone(),
            ..call
        };
        let mut outcome = self.hooks.run(event, &call, &never);
        if let Ok(mut rows) = self.rows.lock() {
            outcome.rows = rows.drain(..).chain(outcome.rows).collect();
        }
        outcome
    }
}

type Named<'a> = (&'a str, &'a str, &'a Map<String, Value>);

impl yi_mcp_cli::CallHooks for McpHooks {
    fn before(
        &self,
        server: &str,
        tool: &str,
        arguments: Map<String, Value>,
    ) -> yi_mcp_cli::Arguments {
        let named = (server, tool, &arguments);
        let outcome = self.run(Event::PreToolUse, named, Call::default());
        let rows: String = (outcome.rows.iter())
            .map(|row| format!("\n{row}"))
            .collect();
        let refused = |why: &str| Err(format!("Claude Code PreToolUse hook: {why}{rows}"));
        if let Some(reason) = &outcome.stop {
            return refused(&format!("the run stops: {reason}"));
        }
        match &outcome.verdict {
            Verdict::Deny(reason) => return refused(reason),
            Verdict::Ask(why) => return refused(&format!("asks ({why}), and a cell cannot ask")),
            Verdict::Allow | Verdict::Silent => {}
        }
        if let Ok(mut pending) = self.rows.lock() {
            pending.extend(outcome.rows);
        }
        Ok(outcome.updated_input.unwrap_or(arguments))
    }

    fn after(&self, named: Named<'_>, outcome: yi_mcp_cli::Outcome) -> yi_mcp_cli::Outcome {
        let mut call = Call::default();
        let event = match &outcome {
            Ok(result) if result.get("isError") != Some(&Value::Bool(true)) => {
                call.response = Some(result.clone());
                Event::PostToolUse
            }
            Ok(result) => {
                call.error = Some(result.to_string());
                Event::PostToolUseFailure
            }
            Err(error) => {
                call.error = Some(error.clone());
                Event::PostToolUseFailure
            }
        };
        let post = self.run(event, named, call);
        let stop =
            (post.stop).map(|reason| format!("[Claude Code hook stopped the run: {reason}]"));
        let rows = post.rows.into_iter().chain(stop);
        match outcome {
            Ok(result) => {
                let mut result = post.updated_output.unwrap_or(result);
                let content = (result.as_object_mut())
                    .map(|fields| fields.entry("content").or_insert_with(|| json!([])));
                if let Some(Value::Array(content)) = content {
                    content.extend(rows.map(|text| json!({"type": "text", "text": text})));
                }
                Ok(result)
            }
            Err(error) => Err(std::iter::once(error)
                .chain(rows)
                .collect::<Vec<_>>()
                .join("\n")),
        }
    }
}
