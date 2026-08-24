use std::collections::BTreeMap;

use serde_json::Value;

/// Design P18: a named, typed portion of model-visible state. A diff is
/// rendered only when the snapshot changed; per-turn re-injection is a diff
/// or it is nothing — the stable prefix is never disturbed by a changed
/// value.
pub trait WorldStateSection: Send + Sync {
    fn name(&self) -> &'static str;
    fn snapshot(&self) -> Value;
    fn render(&self, previous: Option<&Value>) -> Option<String>;
}

#[derive(Default)]
pub struct WorldState {
    sections: Vec<Box<dyn WorldStateSection>>,
    previous: BTreeMap<&'static str, Value>,
}

impl WorldState {
    pub fn add_section(&mut self, section: Box<dyn WorldStateSection>) {
        self.sections.push(section);
    }

    /// Renders every section as new — the full state injected once per
    /// window.
    pub fn render_full(&mut self) -> Vec<String> {
        self.previous.clear();
        self.render_changed()
    }

    /// Renders only sections whose snapshot changed since the last render;
    /// fragments are appended at the overlay tail (P11).
    pub fn render_changed(&mut self) -> Vec<String> {
        let mut fragments = Vec::new();
        for section in &self.sections {
            let snapshot = section.snapshot();
            let previous = self.previous.get(section.name());
            if previous == Some(&snapshot) {
                continue;
            }
            if let Some(fragment) = section.render(previous) {
                fragments.push(fragment);
            }
            self.previous.insert(section.name(), snapshot);
        }
        fragments
    }
}
