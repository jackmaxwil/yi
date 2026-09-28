use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_loop::TurnSnapshot;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::plan::doc::{BlockedOn, Plan, PlanState, TodoState};

use super::ops::OWNER_AGENT;
use crate::goal::StoreHandle;
use crate::session::{AgentSession, InterceptStopFn, PromptChoiceFn, TurnCoupling, TurnObserveFn};

pub const LEDGER_TOOL: &str = "plan";
pub const LEDGER_CUSTOM_TYPE: &str = "ledger_prompt";

/// Every threshold the loop coupling reads is named here and nowhere else,
/// fitted on task-shape features alone, never on a benchmark identity.
pub mod gate {
    pub const STOP_CAP_PER_CYCLE: u32 = 2;
    pub const MULTI_STEP_SCORE: usize = 2;
    pub const LONG_PROMPT_WORDS: usize = 30;
    pub const ENUMERATED_ITEMS_MIN: usize = 2;
    pub const CONJUNCTIONS: [&str; 3] = ["and", "then", "also"];
    pub const INTERROGATIVES: [&str; 10] = [
        "what", "how", "why", "where", "when", "who", "which", "is", "are", "does",
    ];

    fn word(token: &str) -> String {
        token
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    }

    /// The text of a list line (`- `, `* `, `1.`, `1)`), or None for prose.
    pub fn enumerated(line: &str) -> Option<&str> {
        let lead = line.trim_start();
        if let Some(rest) = lead.strip_prefix("- ").or_else(|| lead.strip_prefix("* ")) {
            return Some(rest);
        }
        lead.split_once(['.', ')'])
            .filter(|(head, _)| !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()))
            .map(|(_, rest)| rest)
    }

    /// True for multi-step work: the verdict is advisory to the loop and never
    /// to the model — false suppresses the forced tool_choice, nothing more.
    pub fn eager_init(prompt: &str) -> bool {
        let (trimmed, levers) = (prompt.trim(), crate::levers::get());
        if trimmed.is_empty() || trimmed.ends_with('?') {
            return false;
        }
        let words: Vec<String> = trimmed.split_whitespace().map(word).collect();
        if words
            .first()
            .is_some_and(|first| INTERROGATIVES.contains(&first.as_str()))
        {
            return false;
        }
        if trimmed.lines().filter_map(enumerated).count() >= levers.plan_enumerated_min {
            return true;
        }
        let conjunctions = words
            .iter()
            .filter(|token| CONJUNCTIONS.contains(&token.as_str()))
            .count();
        let extra_sentences = trimmed
            .matches(". ")
            .count()
            .saturating_add(trimmed.matches("! ").count())
            .saturating_add(trimmed.matches("? ").count());
        let long = usize::from(words.len() >= levers.plan_long_prompt_words);
        conjunctions
            .saturating_add(extra_sentences)
            .saturating_add(long)
            >= levers.plan_multi_step_score
    }
}

/// What a terminal turn's ledger state asks of the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopPosture {
    /// Workable todos and nothing to wait on: force the continuation.
    Continue,
    /// Blocked on the user: end the turn and ask.
    Ask,
    /// Blocked on an external condition: check on a cadence, do not spin.
    Cadence,
    /// Finished, inactive, or running children will re-wake the loop.
    Quiet,
}

impl StopPosture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Ask => "ask",
            Self::Cadence => "cadence",
            Self::Quiet => "quiet",
        }
    }
}

pub fn stop_posture(plan: &Plan) -> StopPosture {
    if plan.state != PlanState::Active || plan.finished() {
        return StopPosture::Quiet;
    }
    let mut blocked_on_child = false;
    let mut running_inline = false;
    let mut failed = false;
    for todo in &plan.todos {
        match &todo.state {
            TodoState::Blocked { on, note: _ } => match on {
                BlockedOn::User => return StopPosture::Ask,
                BlockedOn::External { probe: _ } | BlockedOn::Other(_) => {
                    return StopPosture::Cadence;
                }
                BlockedOn::Child(_) => blocked_on_child = true,
            },
            TodoState::Running { by } => {
                if by.as_str() == OWNER_AGENT {
                    running_inline = true;
                }
            }
            TodoState::Failed { cause: _, last: _ } => failed = true,
            TodoState::Pending
            | TodoState::Done { .. }
            | TodoState::Abandoned
            | TodoState::Other(_) => {}
        }
    }
    if !plan.ready().is_empty() || blocked_on_child || running_inline || failed {
        return StopPosture::Continue;
    }
    StopPosture::Quiet
}

/// The windowed re-injection: counts (hidden included) plus the frontier,
/// never the full ledger — the plan itself rehydrates from its file.
pub fn reinjection_text(plan: &Plan) -> String {
    let mut text = super::summary_line(plan);
    let frontier = super::frontier_text(plan);
    if !frontier.is_empty() {
        text.push('\n');
        text.push_str(&frontier);
    }
    text
}

pub fn ledger_message(text: String, display: bool) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: LEDGER_CUSTOM_TYPE.to_owned(),
        content: UserContent::Text(text),
        display,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn asking_user(message: &AgentMessage) -> bool {
    let AgentMessage::Assistant { content, .. } = message else {
        return false;
    };
    content
        .iter()
        .rev()
        .find_map(|block| match block {
            Content::Text { text, .. } => Some(text.trim_end().ends_with('?')),
            Content::Thinking { .. } | Content::Image { .. } | Content::ToolCall { .. } => None,
        })
        .unwrap_or(false)
}

#[derive(Default)]
struct Cycle {
    interceptions: u32,
}

pub struct CouplingOptions {
    pub plans_dir: PathBuf,
}

fn stop_text(plan: &Plan) -> String {
    format!(
        "The turn ended with open todos.\n{}\nContinue the work or step the ledger; block a todo on the user only when truly stuck.",
        reinjection_text(plan)
    )
}

pub fn coupling(session: &AgentSession, options: CouplingOptions) -> TurnCoupling {
    let CouplingOptions { plans_dir } = options;
    let cycle = Arc::new(Mutex::new(Cycle::default()));
    let store: StoreHandle = session.store_handle();
    let on_prompt: Arc<PromptChoiceFn> = {
        let cycle = Arc::clone(&cycle);
        Arc::new(move |prompt: &AgentMessage| {
            if matches!(prompt, AgentMessage::User { .. })
                && let Ok(mut cycle) = cycle.lock()
            {
                cycle.interceptions = 0;
            }
            None
        })
    };
    let on_turn: Arc<TurnObserveFn> = Arc::new(|_snapshot: &TurnSnapshot| {});
    let intercept_stop = {
        let cycle = Arc::clone(&cycle);
        Arc::new(move |snapshot: &TurnSnapshot| -> Option<AgentMessage> {
            if asking_user(snapshot.message) {
                return None;
            }
            if cycle
                .lock()
                .map(|cycle| cycle.interceptions >= crate::levers::get().plan_stop_cap)
                .unwrap_or(true)
            {
                return None;
            }
            // Invariant: synchronous by design — no await point between the terminal turn and
            // the queue read, which the frontmatter cap bounds, so pumps cannot stall.
            let plan = super::canonical_plan(&store, &plans_dir).ok()?;
            if stop_posture(&plan) != StopPosture::Continue {
                return None;
            }
            if let Ok(mut cycle) = cycle.lock() {
                cycle.interceptions = cycle.interceptions.saturating_add(1);
            }
            Some(ledger_message(stop_text(&plan), true))
        }) as Arc<InterceptStopFn>
    };
    TurnCoupling {
        on_prompt,
        on_turn,
        intercept_stop,
    }
}

pub fn install(session: &AgentSession, options: CouplingOptions) {
    session.set_turn_coupling(coupling(session, options));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use yi_types::plan::doc::{AgentId, GoalText, PlanId, PlanTier, Todo, TodoLabel};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn the_gate_agrees_with_every_fixture_prompt() -> Fallible {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans");
        let mut seen = 0usize;
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let prompt = fixture
                .get("prompt")
                .and_then(Value::as_str)
                .ok_or("fixture has no prompt")?;
            let eager = fixture
                .get("eagerInit")
                .and_then(Value::as_bool)
                .ok_or("fixture has no eagerInit")?;
            assert_eq!(gate::eager_init(prompt), eager, "{prompt}");
            seen = seen.saturating_add(1);
        }
        assert_eq!(seen, 3);
        Ok(())
    }

    #[test]
    fn a_question_never_forces_the_ledger() {
        assert!(!gate::eager_init(
            "what does the resolver do with a stale hashline tag?"
        ));
        assert!(!gate::eager_init("how is the plan stored and mirrored"));
        assert!(!gate::eager_init(""));
    }

    #[test]
    fn enumerated_items_alone_read_as_multi_step() {
        assert!(gate::eager_init("do these:\n- fix the build\n- ship it"));
    }

    fn todo(label: &str, state: TodoState) -> Fallible2<Todo> {
        Ok(Todo {
            state,
            ..Todo::pending(TodoLabel::new(label)?)
        })
    }

    type Fallible2<T> = Result<T, Box<dyn std::error::Error>>;

    fn plan_of(todos: Vec<Todo>) -> Fallible2<Plan> {
        Ok(Plan::opening(
            PlanId::new("posture")?,
            GoalText::new("posture cases")?,
            PlanTier::Root,
            todos,
        ))
    }

    #[test]
    fn stop_posture_covers_every_suppression_case() -> Fallible {
        let open = plan_of(vec![todo("ship", TodoState::Pending)?])?;
        assert_eq!(stop_posture(&open), StopPosture::Continue);

        let asking = plan_of(vec![todo(
            "ship",
            TodoState::Blocked {
                on: BlockedOn::User,
                note: "which registry".to_owned(),
            },
        )?])?;
        assert_eq!(stop_posture(&asking), StopPosture::Ask);

        let waiting = plan_of(vec![todo(
            "ship",
            TodoState::Blocked {
                on: BlockedOn::External { probe: None },
                note: "vendor mount".to_owned(),
            },
        )?])?;
        assert_eq!(stop_posture(&waiting), StopPosture::Cadence);

        let child = AgentId::new("posture.ship")?;
        let delegated = plan_of(vec![todo("ship", TodoState::Running { by: child })?])?;
        assert_eq!(
            stop_posture(&delegated),
            StopPosture::Quiet,
            "a running child re-wakes the loop"
        );

        let finished = plan_of(vec![todo(
            "ship",
            TodoState::Done {
                output: None,
                resolution: None,
            },
        )?])?;
        assert_eq!(stop_posture(&finished), StopPosture::Quiet);

        let inline = AgentId::new(OWNER_AGENT)?;
        let working = plan_of(vec![todo("ship", TodoState::Running { by: inline })?])?;
        assert_eq!(stop_posture(&working), StopPosture::Continue);
        Ok(())
    }

    #[test]
    fn a_terminal_question_reads_as_asking_the_user() {
        let message = AgentMessage::Assistant {
            content: vec![Content::Text {
                text: "Should I use the staging registry?".to_owned(),
                text_signature: None,
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: yi_types::message::Usage::unknown(),
            stop_reason: yi_types::message::StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        };
        assert!(asking_user(&message));
    }

    #[test]
    fn the_reinjection_keeps_the_frontier_and_drops_the_rest() -> Fallible {
        let plan = plan_of(vec![
            todo(
                "cut the scope",
                TodoState::Done {
                    output: None,
                    resolution: None,
                },
            )?,
            todo(
                "write the codec",
                TodoState::Done {
                    output: None,
                    resolution: None,
                },
            )?,
            todo("ship the tool", TodoState::Pending)?,
        ])?;
        let text = reinjection_text(&plan);
        assert!(text.contains("3") && text.contains("2 done"), "{text}");
        assert!(text.contains("ship the tool"), "{text}");
        assert!(!text.contains("write the codec"), "{text}");
        let message = ledger_message(text, false);
        assert!(
            yi_context::drop_internal(&[message]).is_empty(),
            "the injection dies at compaction instead of accumulating"
        );
        Ok(())
    }
}
