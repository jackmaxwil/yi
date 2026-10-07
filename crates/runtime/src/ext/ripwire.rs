use super::{Effect, Event, EventMask, Extension, Rank, Slot};

const FRAGMENT: &str = include_str!("../prompts/ripwire.md");

pub struct Ripwire;

impl Extension for Ripwire {
    fn name(&self) -> &'static str {
        "ripwire"
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        if matches!(event, Event::SessionStart { .. }) && yi_tools::ripwire::installed() {
            out.push(Effect::AttachFragment {
                slot: Slot::new(Rank::Tool, "ripwire"),
                text: FRAGMENT.to_owned(),
            });
        }
    }
}
