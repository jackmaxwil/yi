use yi_orb::OrbState;

use super::{App, Bottom};
use crate::cell::{TaskStatus, ToolStatus};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TitleMarker {
    Ready,
    Attention,
    Pulse(&'static str),
}

impl App {
    pub fn working_label(&self) -> Option<String> {
        let state = self.orb_state()?;
        let waiting = self
            .wait
            .as_ref()
            .filter(|_| self.running)
            .map(|(wait, since)| {
                let now = self.clock.now();
                crate::pen::wait_label(wait, *since, now, &self.selection.model.provider)
            });
        Some(waiting.unwrap_or_else(|| state.label().to_owned()))
    }

    pub fn orb_state(&self) -> Option<OrbState> {
        if matches!(
            self.bottom,
            Some(Bottom::Approval(..) | Bottom::Question(..))
        ) {
            return Some(OrbState::Listening);
        }
        if !self.running {
            return None;
        }
        match self.wait.as_ref().map(|(wait, _)| wait) {
            Some(yi_types::event::Wait::Retry { .. }) => return Some(OrbState::Stalled),
            Some(yi_types::event::Wait::Compaction { .. }) => return Some(OrbState::Condensing),
            Some(yi_types::event::Wait::KernelBoot { .. }) => return Some(OrbState::KernelBoot),
            None => {}
        }
        if let Some(tool) = self
            .live_tools
            .iter()
            .rev()
            .find(|t| t.status == ToolStatus::Running)
        {
            return Some(self.tool_state(&tool.name));
        }
        if let Some(pen) = &self.pen {
            return Some(self.tool_state(&pen.name));
        }
        let children = self
            .tasks
            .values()
            .filter(|s| s.cell.status == TaskStatus::Running)
            .count();
        if children > 0 {
            return Some(OrbState::Delegating {
                children: u8::try_from(children).unwrap_or(u8::MAX),
            });
        }
        if !self.live_markdown.is_empty() {
            return Some(OrbState::Composing);
        }
        if !self.live_thought.is_empty() {
            return Some(OrbState::Thinking);
        }
        // Incident: a read or a todo update ends in milliseconds, far inside one exit's lead, so
        // its state never showed; the last tool holds the orb until the model streams again.
        Some(
            self.last_tool
                .as_deref()
                .map_or(OrbState::Awaiting, |name| self.tool_state(name)),
        )
    }

    /// Invariant: the names are the registered tools'; `answer`, `ask_user` and `annotate` are
    /// bookkeeping, not work, so they take the generic state.
    fn tool_state(&self, name: &str) -> OrbState {
        match name {
            "read" | "get_context" => OrbState::Reading,
            "grep" | "web_search" => OrbState::Searching,
            "edit" | "write" => OrbState::Editing,
            "bash" => OrbState::Executing,
            "ipython" => OrbState::Computing,
            "todo" | "plan" => {
                let progress = self.todos.as_ref().map(|todos| todos.progress());
                let count = |n: usize| u8::try_from(n).unwrap_or(u8::MAX);
                OrbState::Planning {
                    done: progress.map_or(0, |p| count(p.done)),
                    total: progress.map_or(0, |p| count(p.total)),
                }
            }
            _ => OrbState::Working,
        }
    }

    pub(crate) fn refresh_title(&mut self) {
        let marker = match self.orb_state() {
            None => TitleMarker::Ready,
            Some(yi_orb::OrbState::Listening) => TitleMarker::Attention,
            Some(
                yi_orb::OrbState::Awaiting
                | yi_orb::OrbState::Thinking
                | yi_orb::OrbState::Composing
                | yi_orb::OrbState::Working
                | yi_orb::OrbState::Reading
                | yi_orb::OrbState::Searching
                | yi_orb::OrbState::Browsing
                | yi_orb::OrbState::Editing
                | yi_orb::OrbState::Executing
                | yi_orb::OrbState::Computing
                | yi_orb::OrbState::Planning { .. }
                | yi_orb::OrbState::Delegating { .. }
                | yi_orb::OrbState::Stalled
                | yi_orb::OrbState::Condensing
                | yi_orb::OrbState::KernelBoot,
            ) => TitleMarker::Pulse(crate::motion::tab_title_pulse(
                self.clock.since(self.turn_started),
            )),
        };
        if self.title_marker == Some(marker) {
            return;
        }
        self.title_marker = Some(marker);
        self.pending_title = Some(match marker {
            TitleMarker::Ready => self.base_title.clone(),
            TitleMarker::Attention => format!("! {}", self.base_title),
            TitleMarker::Pulse(frame) => format!("{frame} {}", self.base_title),
        });
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    pub(crate) fn update_prompt_title(&mut self, focus: &str) {
        let title = format!("Yi — {focus}");
        if self.base_title != title {
            self.base_title = title;
            self.title_marker = None;
            self.refresh_title();
        }
    }
}
