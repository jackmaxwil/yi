//! the kernel's constants behind one struct, overridable from a file in an eval run only (D220).

use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::ext::orchestrate as route;
use crate::plan::loop_coupling::gate as plan_gate;
use crate::todo::coupling::gate as todo_gate;
use crate::{auto_review, family, lane, mailbox, plan, subagent};

/// The one env var the levers own: a path to a JSON object of `{"<key>": <integer>}`.
pub const ENV: &str = "YI_LEVERS";

/// A lever as `evals/levers/levers.json` lists it; `levers::the_manifest_matches` holds them equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub key: &'static str,
    pub min: i64,
    pub max: i64,
    pub tunable: bool,
}

macro_rules! levers {
    ($($field:ident: $ty:ty = $key:literal, $default:expr, $min:literal..=$max:literal, $tunable:literal;)*) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Levers { $(pub $field: $ty,)* }

        impl Levers {
            /// Invariant: each default is the owning module's own constant, so an unset lever
            /// reads exactly what the module read before the struct existed.
            pub const DEFAULT: Self = Self { $($field: $default,)* };
            pub const SPECS: &'static [Spec] =
                &[$(Spec { key: $key, min: $min, max: $max, tunable: $tunable },)*];

            pub fn to_value(&self) -> Value {
                let mut map = Map::new();
                $(map.insert($key.to_owned(), Value::from(self.$field));)*
                Value::Object(map)
            }

            /// Invariant: an override is refused, never clamped: an unknown key, a lever that
            /// is not tunable, a value that is not an integer, a value outside the range.
            fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
                match key {
                    $($key => {
                        if !$tunable {
                            return Err(format!("{key} is not tunable"));
                        }
                        let number = value
                            .as_i64()
                            .filter(|number| ($min..=$max).contains(number))
                            .ok_or_else(|| format!("{key} wants an integer in {}..={}, not {value}", $min, $max))?;
                        self.$field = <$ty>::try_from(number).map_err(|error| format!("{key}: {error}"))?;
                    })*
                    _ => return Err(format!("unknown lever {key}")),
                }
                Ok(())
            }
        }
    };
}

levers! {
    plan_width_max: usize = "plan.width_max", plan::ops::WIDTH_MAX, 1..=16, true;
    plan_spawn_cap: u32 = "plan.spawn_cap", yi_types::plan::doc::SPAWN_CAP.get(), 8..=256, false;
    plan_retry_cap: u8 = "plan.retry_cap", plan::table::RETRY_CAP.0, 1..=16, false;
    plan_stale_turns: u64 = "plan.stale_turns", plan::DEFAULT_STALE_TURNS, 4..=40, true;
    plan_probe_first_s: u64 = "plan.probe_first_s", crate::schedule::clock::PROBE_EVERY_S, 10..=600, true;
    plan_probe_max_s: u64 = "plan.probe_max_s", crate::schedule::clock::PROBE_MAX_S, 300..=7200, true;
    plan_stop_cap: u32 = "plan.stop_cap", plan_gate::STOP_CAP_PER_CYCLE, 0..=6, true;
    plan_multi_step_score: usize = "plan.multi_step_score", plan_gate::MULTI_STEP_SCORE, 1..=6, true;
    plan_long_prompt_words: usize = "plan.long_prompt_words", plan_gate::LONG_PROMPT_WORDS, 10..=120, true;
    plan_enumerated_min: usize = "plan.enumerated_min", plan_gate::ENUMERATED_ITEMS_MIN, 1..=6, true;
    plan_done_refusal_cap: u32 = "plan.done_refusal_cap", yi_types::plan::contract::DONE_REFUSAL_CAP, 1..=8, true;
    plan_judge_cap: u32 = "plan.judge_cap", yi_types::plan::contract::JUDGE_CAP_PER_TODO, 1..=6, true;
    plan_jury: u32 = "plan.jury", 1, 1..=3, false;
    plan_scatter_rounds: u32 = "plan.scatter_rounds", 3, 1..=6, false;
    todo_nudge_work: u32 = "todo.nudge_work", todo_gate::NUDGE_WORK, 4..=40, true;
    todo_quiet_turns: u32 = "todo.quiet_turns", todo_gate::QUIET_TURNS, 1..=10, true;
    todo_first_list_work: u32 = "todo.first_list_work", todo_gate::FIRST_LIST_WORK, 1..=10, true;
    todo_nudge_cap: u32 = "todo.nudge_cap", todo_gate::NUDGE_CAP_PER_CYCLE, 0..=6, true;
    todo_intercept_cap: u32 = "todo.intercept_cap", todo_gate::INTERCEPT_CAP_PER_CYCLE, 0..=12, true;
    todo_empty_stop_cap: u32 = "todo.empty_stop_cap", todo_gate::EMPTY_STOP_CAP, 1..=8, true;
    todo_ladder_top: u8 = "todo.ladder_top", todo_gate::LADDER_TOP, 1..=3, false;
    family_max_children: usize = "family.max_children", subagent::DEFAULT_MAX_CHILDREN, 2..=128, true;
    family_cap: usize = "family.cap", subagent::FAMILY_CAP, 4..=128, true;
    family_depth: u8 = "family.depth", yi_types::config::DEFAULT_MAX_DEPTH, 1..=3, false;
    family_stuck_idle_s: u64 = "family.stuck_idle_s", family::STUCK_IDLE_MS / 1000, 60..=1800, true;
    mail_wait_min_ms: u64 = "mail.wait_min_ms", mailbox::WAIT_MIN_MS, 100..=10000, false;
    mail_wait_max_ms: u64 = "mail.wait_max_ms", mailbox::WAIT_MAX_MS, 10000..=3600000, false;
    mail_context_keys: usize = "mail.context_keys", mailbox::CONTEXT_MAX_KEYS, 1..=32, false;
    mail_context_value: usize = "mail.context_value", mailbox::CONTEXT_VALUE_CAP, 512..=65536, false;
    mail_context_total: usize = "mail.context_total", mailbox::CONTEXT_TOTAL_CAP, 2048..=262144, false;
    mail_discoveries: usize = "mail.discoveries", mailbox::MAX_DISCOVERIES, 1..=64, false;
    lane_slots: u8 = "lane.slots", lane::DEFAULT_SLOTS, 1..=255, false;
    route_complex_at: i32 = "route.complex_at", route::COMPLEX_AT, 1..=10, true;
    route_oneshot_at: i32 = "route.oneshot_at", route::ONESHOT_AT, -6..=0, true;
    route_tool_calls_per_turn: u32 = "route.tool_calls_per_turn", route::TOOL_CALLS_PER_TURN, 2..=12, true;
    route_files_matched: u32 = "route.files_matched", route::FILES_MATCHED, 2..=20, true;
    loop_length_stop_at: u32 = "loop.length_stop_at", yi_loop::LENGTH_STOP_AT, 1..=6, true;
    loop_cut_stop_at: u32 = "loop.cut_stop_at", yi_loop::CUT_STOP_AT, 1..=24, true;
    loop_repeat_steer_at: u32 = "loop.repeat_steer_at", yi_loop::REPEAT_STEER_AT, 1..=6, false;
    loop_repeat_stop_at: u32 = "loop.repeat_stop_at", yi_loop::REPEAT_STOP_AT, 2..=12, false;
    loop_reasoning_cap: usize = "loop.reasoning_cap", yi_loop::REASONING_CHAR_CAP, 8000..=200000, true;
    tools_reduce_floor: usize = "tools.reduce_floor", yi_tools::reduce::REDUCE_FLOOR, 2048..=32768, false;
    review_timeout_s: u64 = "review.timeout_s", auto_review::REVIEW_TIMEOUT.as_secs(), 10..=120, true;
    graph_next_lines: usize = "graph.next_lines", crate::todo::text::NEXT_LINES, 1..=5, true;
}

impl Default for Levers {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Levers {
    /// The loop's stop guards (D280): yi-loop sits below the runtime and never reads a lever,
    /// so the session hands it these through `LoopConfig`.
    pub fn loop_guards(&self) -> yi_loop::LoopGuards {
        yi_loop::LoopGuards {
            length_stop_at: self.loop_length_stop_at,
            cut_stop_at: self.loop_cut_stop_at,
            reasoning_cap: self.loop_reasoning_cap,
        }
    }

    /// Invariant: outside an eval run the variable is never asked for, so no file is opened
    /// and nothing is parsed; inside one a refused file is an error, never silent defaults.
    pub fn load(eval: bool, var: impl FnOnce() -> Option<OsString>) -> Result<Self, String> {
        let Some(path) = eval.then(var).flatten() else {
            return Ok(Self::DEFAULT);
        };
        let path = Path::new(&path);
        let read = || -> Result<Self, String> {
            let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            let overrides: Map<String, Value> =
                serde_json::from_str(&text).map_err(|error| error.to_string())?;
            let mut levers = Self::DEFAULT;
            for (key, value) in &overrides {
                levers.set(key, value)?;
            }
            Ok(levers)
        };
        read().map_err(|error| format!("{ENV}={}: {error}", path.display()))
    }
}

static LEVERS: OnceLock<Levers> = OnceLock::new();

/// Once per process, from `build_session`'s first statement, before a session exists and
/// so before any lever is read: an eval run is one that carries `--eval`, a harness's flag.
pub fn init(eval: bool) -> Result<(), String> {
    let path = eval.then(|| std::env::var_os(ENV)).flatten();
    let levers = Levers::load(eval, || path.clone())?;
    // Invariant: a run that is not on the defaults says so once, on stderr and never in a
    // model-facing byte, so no measurement is read as the shipped configuration.
    if let Some(path) = path.filter(|_| levers != Levers::DEFAULT) {
        eprintln!("levers: this run reads {}", Path::new(&path).display());
    }
    LEVERS.get_or_init(|| levers);
    Ok(())
}

pub fn get() -> &'static Levers {
    LEVERS.get().unwrap_or(&Levers::DEFAULT)
}
