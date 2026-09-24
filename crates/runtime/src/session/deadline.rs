//! `--deadline` as one clock: the environment line, the tool cancel and the stop all read it.

use std::time::{Duration, Instant};

/// The longest request measured ran about 170 s; one started inside the margin must finish.
const STOP_MARGIN: Duration = Duration::from_secs(240);
const LAST_WORD_CAP: Duration = Duration::from_secs(90);
const LAST_WORD_FLOOR: Duration = Duration::from_secs(30);
const SHUTDOWN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    pub(crate) started: Instant,
    pub(crate) total: Duration,
}

impl Deadline {
    pub(super) fn new(total: Duration) -> Self {
        Self {
            started: Instant::now(),
            total,
        }
    }

    pub(super) fn passed(self, margin: Duration) -> bool {
        self.started.elapsed().saturating_add(margin) >= self.total
    }

    /// No turn starts in the last `STOP_MARGIN`; a short budget keeps three quarters of itself.
    pub(crate) fn winding_down(self) -> bool {
        self.passed(STOP_MARGIN.min(self.total / 4))
    }

    /// Incident: a grace sized off quick tool-call turns cut the last word 5 s into its answer.
    pub(super) fn last_word_due(self, longest_answer: Option<Duration>) -> bool {
        let cap = LAST_WORD_CAP.min(self.total / 8);
        let answer = longest_answer.map_or(cap, |turn| turn.max(LAST_WORD_FLOOR.min(cap)).min(cap));
        self.passed(answer.saturating_add(SHUTDOWN.min(self.total / 16)))
    }
}

impl super::Shared {
    pub(super) fn last_word_due(&self) -> bool {
        let longest = self.turn_time.lock().ok().and_then(|clock| clock.1);
        self.deadline
            .get()
            .is_some_and(|clock| clock.last_word_due(longest))
    }

    pub(super) fn time_turn(&self, event: &yi_types::event::AgentEvent) {
        use yi_types::event::AgentEvent;
        let Ok(mut clock) = self.turn_time.lock() else {
            return;
        };
        match event {
            AgentEvent::TurnStart => clock.0 = Some(Instant::now()),
            AgentEvent::MessageEnd {
                message: yi_types::message::AgentMessage::Assistant { content, .. },
            } => {
                let answer = content
                    .iter()
                    .any(|block| matches!(block, yi_types::message::Content::Text { .. }));
                if let Some(started) = clock.0.take().filter(|_| answer) {
                    clock.1 = clock.1.max(Some(started.elapsed()));
                }
            }
            _ => {}
        }
    }

    /// The loop's check at its message boundary: out of clock, or cancelled by the parent.
    pub(super) fn winding_down(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
            || self
                .deadline
                .get()
                .is_some_and(|clock| clock.winding_down())
    }
}

impl super::AgentSession {
    /// A `cancel` from the parent: the turn in flight settles and no request follows it.
    pub(crate) fn cancel(&self) {
        let flag = &self.shared.cancelled;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.shared
            .cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_ai::faux::{faux_assistant_message, faux_text, faux_thinking, faux_tool_call};
    use yi_types::message::{AgentMessage, StopReason};

    fn begun(ago: u64, total: u64) -> Deadline {
        Deadline {
            started: Instant::now()
                .checked_sub(Duration::from_secs(ago))
                .unwrap(),
            total: Duration::from_secs(total),
        }
    }

    /// A flat 240 s margin is a 120 s fixture's whole budget: one turn per run.
    #[test]
    fn the_stop_margin_is_four_minutes_or_a_quarter_of_the_budget() {
        assert!(!begun(80, 120).winding_down());
        assert!(begun(91, 120).winding_down());
        assert!(!begun(750, 1000).winding_down());
        assert!(begun(761, 1000).winding_down());
        assert!(!begun(761, 1000).passed(Duration::ZERO));
    }

    /// Dies with the last word started at the deadline itself: `yi ask`'s caller killed the
    /// run as the tool-free turn began, so the answer and the lane release were lost.
    #[test]
    fn the_last_word_reserves_the_longest_turn_and_the_shutdown_capped() {
        let quick = Some(Duration::from_secs(2));
        assert!(
            !begun(379, 420).last_word_due(quick),
            "a 30 s floor and 10 s on top"
        );
        assert!(begun(381, 420).last_word_due(quick));
        let long = Some(Duration::from_secs(45));
        assert!(!begun(944, 1000).last_word_due(long) && begun(946, 1000).last_word_due(long));
        assert!(
            begun(901, 1000).last_word_due(None),
            "unmeasured reserves the 90 s cap"
        );
        assert!(!begun(899, 1000).last_word_due(None));
        assert!(
            begun(98, 120).last_word_due(quick),
            "a short budget caps it at an eighth"
        );
        assert!(!begun(97, 120).last_word_due(quick));
    }

    /// A faux session whose clock began `ago` of `total` seconds back, its model paced.
    fn late_session(script: Vec<AgentMessage>, ago: u64, total: u64) -> super::super::AgentSession {
        let provider = std::sync::Arc::new(crate::provider::ProviderStream::new(None, None));
        provider.queue_faux(script);
        if let Ok(mut pace) = provider.faux_pace.lock() {
            *pace = Some(Duration::from_millis(500));
        }
        let config = super::super::SessionConfig {
            system_prompt: "sys".to_owned(),
            model: crate::plan::dispatch::tests::faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        };
        let mut session = super::super::AgentSession::new(config, provider);
        session.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
        let _first = session.shared.deadline.set(begun(ago, total));
        session
    }

    fn texts(session: &super::super::AgentSession) -> Vec<(StopReason, String)> {
        let assistant = |message: &AgentMessage| match message {
            AgentMessage::Assistant { stop_reason, .. } => {
                Some((*stop_reason, message.plain_text()))
            }
            _ => None,
        };
        session.messages().iter().filter_map(assistant).collect()
    }

    /// Dies with the last word cut 5 s into its answer (the final confirmation's r2 steer): a
    /// grace measured off quick tool-call turns left it 10 s, and `yi ask` took those too.
    #[tokio::test]
    async fn a_last_word_that_streams_for_twenty_seconds_lands_inside_the_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let args = serde_json::json!({"command": "sleep 1000"});
        let call = faux_tool_call("c1", "bash", args.as_object().cloned().unwrap_or_default());
        let answer = "a sixteen chars.".repeat(40);
        let script = vec![
            faux_assistant_message(vec![call], StopReason::ToolUse),
            faux_assistant_message(vec![faux_text(&answer)], StopReason::Stop),
        ];
        let session = late_session(script, 340, 420);
        session.prompt("go")?;
        session.wait_idle().await;
        let ended = session
            .deadline()
            .map(|clock| clock.started.elapsed())
            .unwrap_or_default();
        assert!(
            ended < Duration::from_secs(410),
            "the answer ended at {ended:?}, past 410 s"
        );
        assert_eq!(texts(&session).last(), Some(&(StopReason::Stop, answer)));
        Ok(())
    }

    /// Dies with a model call left streaming when the last word was due (r2 fanout: 273 s of
    /// reasoning, then aborted with no output): the stream is cut and the last word follows.
    #[tokio::test]
    async fn a_stream_still_running_when_the_last_word_is_due_is_cut_for_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let endless = "thinking on. ".repeat(100_000);
        let script = vec![
            faux_assistant_message(vec![faux_thinking(&endless)], StopReason::Stop),
            faux_assistant_message(vec![faux_text("the last word")], StopReason::Stop),
        ];
        let session = late_session(script, 1_000, 1_100);
        session.prompt("go")?;
        let ran = tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await;
        assert!(ran.is_ok(), "the endless stream was never cut");
        let said = texts(&session);
        assert_eq!(
            said.first().map(|(stop, _)| *stop),
            Some(StopReason::Aborted),
            "{said:?}"
        );
        assert_eq!(
            said.last(),
            Some(&(StopReason::Stop, "the last word".to_owned()))
        );
        Ok(())
    }
}
