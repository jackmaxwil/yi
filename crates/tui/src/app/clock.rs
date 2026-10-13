use super::App;
use crate::motion::Clock;

impl App {
    /// Pins every on-screen duration to `clock`; a frozen clock starts the spinner at phase 0.
    pub fn set_clock(&mut self, clock: Clock) {
        if self.clock == Clock::Real && clock != Clock::Real {
            self.started_at = clock.now();
            self.turn_started = clock.now();
        }
        self.clock = clock;
    }

    pub fn advance_clock(&mut self, by: std::time::Duration) {
        self.clock.advance(by);
    }

    pub fn spinner_phase(&self) -> usize {
        usize::try_from(self.clock.since(self.started_at).as_millis() / super::SPINNER_PERIOD_MS)
            .unwrap_or(0)
    }

    /// A drive may not move time while the provider owes the screen something: a model request
    /// the loop announced, or a stream event it has sent that the runtime has not delivered.
    pub fn stream_owed(&self, requests: u64, updates: u64) -> bool {
        self.pacing.turns < self.started_turns
            || self.pacing.turns > requests
            || self.pacing.arrived < updates
    }

    /// The streaming message's end has arrived: reduced, or held behind a reveal a frozen clock
    /// has not moved yet.
    pub fn stream_settled(&self) -> bool {
        self.streaming.is_none()
            || matches!(
                self.pacing.held.front(),
                Some(crate::app::UiEvent::Agent(
                    yi_types::event::AgentEvent::MessageEnd { .. }
                ))
            )
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn has_run(&self) -> bool {
        self.seen_turn
    }

    /// A prompt has been submitted whose turn has not started yet.
    pub fn awaiting_turn(&self) -> bool {
        self.submitted_turns > self.started_turns
    }

    pub fn turn_idle(&self) -> bool {
        self.seen_turn && !self.running && !self.awaiting_turn()
    }
}
