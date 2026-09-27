use std::time::Instant;

use serde_json::Value;
use yi_types::event::Wait;

use crate::cell::{ToolCell, ToolStatus};
use crate::motion::elapsed_ms;
use crate::transcript::arg_summary;

#[derive(Debug, Clone)]
pub struct Pen {
    pub(crate) index: usize,
    pub(crate) name: String,
    pub(crate) raw: String,
    since: Instant,
}

impl Pen {
    pub fn new(index: usize, name: Option<String>) -> Self {
        Self {
            index,
            name: name.unwrap_or_default(),
            raw: String::new(),
            since: Instant::now(),
        }
    }

    pub fn card(&self) -> ToolCell {
        let args = Value::Object(yi_types::json_salvage::parse_streaming_json(&self.raw));
        let body = args
            .as_object()
            .into_iter()
            .flat_map(|args| args.values())
            .filter_map(Value::as_str)
            .max_by_key(|text| text.len())
            .unwrap_or_default();
        let lines: Vec<&str> = body.lines().collect();
        let tail = lines.len().saturating_sub(3);
        ToolCell {
            name: self.name.clone(),
            call_id: String::new(),
            intent: None,
            status: ToolStatus::Running,
            summary: ToolCell::summary_of(&self.name, &arg_summary(&self.name, &args)),
            digest: (lines.len() > 1).then(|| format!("{} lines so far", lines.len())),
            preview: lines
                .iter()
                .skip(tail)
                .map(|line| (*line).to_owned())
                .collect(),
            elapsed_ms: elapsed_ms(self.since),
            calls: 1,
            details: Value::Null,
        }
    }
}

pub fn wait_label(wait: &Wait, since: Instant, provider: &str) -> String {
    let secs = since.elapsed().as_secs();
    match wait {
        Wait::Retry {
            attempt,
            of,
            delay_ms,
            cause,
        } => {
            let left = delay_ms
                .saturating_sub(u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX))
                .div_ceil(1000);
            format!("Retrying · {provider} {cause} · {attempt} of {of} in {left} s")
        }
        Wait::Compaction { tokens } => {
            format!(
                "Condensing · {} tokens · {secs} s",
                crate::status::fmt_tokens(*tokens)
            )
        }
        Wait::KernelBoot { step } => {
            let step: String = step.chars().take(48).collect();
            format!("Starting the kernel · {step} · {secs} s")
        }
    }
}
