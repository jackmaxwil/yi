use serde_json::json;

use super::{Effect, Event, EventMask, Extension};

pub struct RouteTelemetry {
    tool_calls: u32,
}

impl RouteTelemetry {
    pub fn new() -> Self {
        Self { tool_calls: 0 }
    }
}

impl Default for RouteTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl Extension for RouteTelemetry {
    fn name(&self) -> &'static str {
        "route-telemetry"
    }

    fn interests(&self) -> EventMask {
        EventMask::TOOL_CALL
            .with(EventMask::TURN_END)
            .with(EventMask::USAGE)
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        match event {
            Event::ToolCall { .. } => self.tool_calls = self.tool_calls.saturating_add(1),
            Event::TurnEnd {
                turn,
                tool_calls_this_turn,
            } => out.push(Effect::Record {
                key: "turn",
                value: json!({
                    "turn": turn,
                    "tool_calls_this_turn": tool_calls_this_turn,
                    "tool_calls_total": self.tool_calls,
                }),
            }),
            Event::Usage {
                input,
                cache_read,
                cache_write,
            } => {
                let denominator = input.saturating_add(*cache_read);
                let ratio = if denominator > 0 {
                    f64::from(u32::try_from(*cache_read).unwrap_or(u32::MAX))
                        / f64::from(u32::try_from(denominator).unwrap_or(u32::MAX))
                } else {
                    0.0
                };
                out.push(Effect::Record {
                    key: "cache",
                    value: json!({
                        "input": input,
                        "cache_read": cache_read,
                        "cache_write": cache_write,
                        "read_ratio": ratio,
                    }),
                });
            }
            _ => {}
        }
    }
}
