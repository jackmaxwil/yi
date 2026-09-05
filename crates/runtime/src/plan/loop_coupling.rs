use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_loop::TurnSnapshot;
use yi_types::message::{AgentMessage, Attribution, Content, UserContent};
use yi_types::model::{ForcedTool, ToolChoice};
use yi_types::plan::doc::{BlockedOn, Plan, PlanState, TodoState};

use super::ops::OWNER_AGENT;
use crate::goal::StoreHandle;
use crate::session::{AgentSession, InterceptStopFn, PromptChoiceFn, TurnCoupling, TurnObserveFn};

pub const LEDGER_TOOL: &str = "plan";
pub const LEDGER_CUSTOM_TYPE: &str = "ledger_prompt";

/// Every threshold the loop coupling reads is named here and nowhere else,
/// fitted on task-shape features alone, never on a benchmark identity.
pub mod gate {
    /// Mutating tool calls since the last ledger touch that earn one nudge.
    pub const NUDGE_THRESHOLD: u32 = 12;
    pub const NUDGE_CAP_PER_CYCLE: u32 = 2;
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

    fn enumerated(line: &str) -> bool {
        let lead = line.trim_start();
        lead.starts_with("- ")
            || lead.starts_with("* ")
            || lead.split_once(['.', ')']).is_some_and(|(head, _)| {
                !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit())
            })
    }

    /// True for multi-step work: the verdict is advisory to the loop and never
    /// to the model — false suppresses the forced tool_choice, nothing more.
    pub fn eager_init(prompt: &str) -> bool {
        let trimmed = prompt.trim();
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
        if trimmed.lines().filter(|line| enumerated(line)).count() >= ENUMERATED_ITEMS_MIN {
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
        let long = usize::from(words.len() >= LONG_PROMPT_WORDS);
        conjunctions
            .saturating_add(extra_sentences)
            .saturating_add(long)
            >= MULTI_STEP_SCORE
    }
}

/// Divergence between work done and ledger stepped, as an event count, never a timer — a
/// different quantity from [`super::DEFAULT_STALE_TURNS`], which counts quiet turns.
#[derive(Debug, Default)]
pub struct NudgeState {
    counted: u32,
    fired: u32,
}

impl NudgeState {
    pub fn reset_cycle(&mut self) {
        self.counted = 0;
        self.fired = 0;
    }

    pub fn touch(&mut self) {
        self.counted = 0;
    }

    pub fn work(&mut self, mutating_calls: u32) -> bool {
        self.counted = self.counted.saturating_add(mutating_calls);
        if self.counted >= gate::NUDGE_THRESHOLD && self.fired < gate::NUDGE_CAP_PER_CYCLE {
            self.fired = self.fired.saturating_add(1);
            self.counted = 0;
            return true;
        }
        false
    }

    pub fn fired(&self) -> u32 {
        self.fired
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
            | TodoState::Done { output: _ }
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TurnWork {
    pub mutating: u32,
    pub ledger_touched: bool,
}

/// One count per host tool call that mutated; kernel-internal work is however
/// many probes one `ipython` cell ran, so it never spends the nudge budget.
pub fn classify_turn(results: &[AgentMessage], mutating_tools: &HashSet<String>) -> TurnWork {
    let mut work = TurnWork::default();
    for result in results {
        let AgentMessage::ToolResult {
            tool_name,
            is_error,
            ..
        } = result
        else {
            continue;
        };
        if *is_error {
            continue;
        }
        if tool_name == LEDGER_TOOL {
            work.ledger_touched = true;
        } else if mutating_tools.contains(tool_name) {
            work.mutating = work.mutating.saturating_add(1);
        }
    }
    work
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
    nudge: NudgeState,
    interceptions: u32,
}

pub struct CouplingOptions {
    pub plans_dir: PathBuf,
    /// Write and Exec tools, minus kernel-internal `ipython` and the ledger.
    pub mutating_tools: HashSet<String>,
}

pub fn mutating_tool_names(tools: &[Arc<dyn yi_tools::Tool>]) -> HashSet<String> {
    tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.kind(),
                yi_tools::ToolKind::Write | yi_tools::ToolKind::Exec
            ) && tool.name() != "ipython"
        })
        .map(|tool| tool.name().to_owned())
        .collect()
}

fn nudge_text() -> String {
    format!(
        "{} mutating tool calls since the last plan-ledger touch. Reconcile the ledger with what actually happened — batch the op with your next real work, never as a solo ledger call.",
        gate::NUDGE_THRESHOLD
    )
}

fn stop_text(plan: &Plan) -> String {
    format!(
        "The turn ended with open todos.\n{}\nContinue the work or step the ledger; block a todo on the user only when truly stuck.",
        reinjection_text(plan)
    )
}

fn on_prompt_hook(cycle: Arc<Mutex<Cycle>>) -> Arc<PromptChoiceFn> {
    Arc::new(move |prompt: &AgentMessage| {
        let AgentMessage::User {
            content,
            attribution: Attribution::User,
            ..
        } = prompt
        else {
            return None;
        };
        if let Ok(mut cycle) = cycle.lock() {
            cycle.nudge.reset_cycle();
            cycle.interceptions = 0;
        }
        let UserContent::Text(text) = content else {
            return None;
        };
        if !gate::eager_init(text) {
            return None;
        }
        ForcedTool::new(LEDGER_TOOL).ok().map(ToolChoice::Tool)
    })
}

pub fn install(session: &AgentSession, options: CouplingOptions) {
    let CouplingOptions {
        plans_dir,
        mutating_tools,
    } = options;
    let cycle = Arc::new(Mutex::new(Cycle::default()));
    let deliver = session.advisory_hook();
    let store: StoreHandle = session.store_handle();

    let on_turn = {
        let cycle = Arc::clone(&cycle);
        let deliver = Arc::clone(&deliver);
        Arc::new(move |snapshot: &TurnSnapshot| {
            let work = classify_turn(snapshot.tool_results, &mutating_tools);
            let Ok(mut cycle) = cycle.lock() else {
                return;
            };
            if work.ledger_touched {
                cycle.nudge.touch();
            } else if work.mutating > 0 && cycle.nudge.work(work.mutating) {
                deliver(ledger_message(nudge_text(), false));
            }
        }) as Arc<TurnObserveFn>
    };

    let intercept_stop = {
        let cycle = Arc::clone(&cycle);
        Arc::new(move |snapshot: &TurnSnapshot| -> Option<AgentMessage> {
            if asking_user(snapshot.message) {
                return None;
            }
            if cycle
                .lock()
                .map(|cycle| cycle.interceptions >= gate::STOP_CAP_PER_CYCLE)
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

    session.set_turn_coupling(TurnCoupling {
        on_prompt: on_prompt_hook(cycle),
        on_turn,
        intercept_stop,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use yi_types::plan::doc::{AgentId, GoalText, PlanId, PlanTier, RetryCount, Todo, TodoLabel};

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn the_gate_agrees_with_every_fixture_prompt() -> Fallible {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans");
        let mut seen = 0usize;
        for entry in std::fs::read_dir(dir)? {
            let fixture: Value = serde_json::from_str(&std::fs::read_to_string(entry?.path())?)?;
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

    #[test]
    fn the_nudge_fires_at_the_threshold_and_caps_at_twice() {
        let mut nudge = NudgeState::default();
        assert!(!nudge.work(gate::NUDGE_THRESHOLD - 1));
        assert!(nudge.work(1), "the threshold call fires");
        assert_eq!(nudge.fired(), 1);
        nudge.touch();
        assert!(!nudge.work(1), "a touch resets the count");
        assert!(nudge.work(gate::NUDGE_THRESHOLD), "second firing");
        assert!(
            !nudge.work(gate::NUDGE_THRESHOLD * 3),
            "capped at {} per cycle",
            gate::NUDGE_CAP_PER_CYCLE
        );
        nudge.reset_cycle();
        assert!(nudge.work(gate::NUDGE_THRESHOLD), "a new cycle re-arms");
    }

    fn result(name: &str, is_error: bool) -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: "call".to_owned(),
            tool_name: name.to_owned(),
            content: Vec::new(),
            details: None,
            usage: None,
            added_tool_names: None,
            is_error,
            timestamp: 0,
        }
    }

    #[test]
    fn kernel_internal_work_and_errors_never_count() {
        let mutating: HashSet<String> = ["write", "edit", "bash"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let work = classify_turn(
            &[
                result("ipython", false),
                result("read", false),
                result("write", false),
                result("write", true),
                result("plan", false),
            ],
            &mutating,
        );
        assert_eq!(
            work,
            TurnWork {
                mutating: 1,
                ledger_touched: true
            }
        );
    }

    fn todo(label: &str, state: TodoState) -> Fallible2<Todo> {
        Ok(Todo {
            label: TodoLabel::new(label)?,
            after: Vec::new(),
            state,
            delegation: None,
            subplan: None,
            retries: RetryCount::default(),
            children: Vec::new(),
            extra: serde_json::Map::new(),
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

        let finished = plan_of(vec![todo("ship", TodoState::Done { output: None })?])?;
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
            todo("cut the scope", TodoState::Done { output: None })?,
            todo("write the codec", TodoState::Done { output: None })?,
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
