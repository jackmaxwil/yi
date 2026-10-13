use std::path::PathBuf;

use yi_tools::plugin::{PLUGINS, Shape};

use super::{Effect, Event, EventMask, Extension, Rank, Slot};

pub struct Plugins {
    pub(super) home: PathBuf,
}

impl Extension for Plugins {
    fn name(&self) -> &'static str {
        "plugins"
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        if !matches!(event, Event::SessionStart { .. }) {
            return;
        }
        for plugin in PLUGINS
            .iter()
            .filter(|plugin| plugin.shape == Shape::Program && plugin.found(&self.home).is_some())
        {
            out.push(Effect::AttachFragment {
                slot: Slot::new(Rank::Tool, plugin.name),
                text: plugin.card.to_owned(),
            });
        }
    }
}
