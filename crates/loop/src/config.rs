use yi_types::message::AgentMessage;
use yi_types::model::{Effort, Model, ToolChoice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

pub struct TurnSnapshot<'a> {
    pub message: &'a AgentMessage,
    pub tool_results: &'a [AgentMessage],
}

#[derive(Debug, Clone, Default)]
pub struct NextTurn {
    pub model: Option<Model>,
    pub thinking: Option<Effort>,
}

type ConvertFn = dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync;
type TailFn = dyn Fn() -> Vec<AgentMessage> + Send + Sync;
type StopFn = dyn Fn(&TurnSnapshot) -> bool + Send + Sync;
type WaitingFn = dyn Fn() -> u64 + Send + Sync;
type PrepareFn = dyn Fn(&TurnSnapshot) -> Option<NextTurn> + Send + Sync;
type QueueFn = dyn Fn() -> Vec<AgentMessage> + Send + Sync;
type InterceptFn = dyn Fn(&TurnSnapshot) -> Option<AgentMessage> + Send + Sync;
type CompactFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<AgentMessage>>> + Send>>;
type CompactFn = dyn Fn(&[AgentMessage]) -> CompactFuture + Send + Sync;
pub type GateFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
type GateFn = dyn Fn() -> GateFuture + Send + Sync;

/// The three stop guards an eval run may move (D280): the loop reads them from here, and the
/// runtime copies them from its levers. Defaults are the crate's constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopGuards {
    pub length_stop_at: u32,
    pub cut_stop_at: u32,
    pub reasoning_cap: usize,
}

impl Default for LoopGuards {
    fn default() -> Self {
        Self {
            length_stop_at: crate::LENGTH_STOP_AT,
            cut_stop_at: crate::CUT_STOP_AT,
            reasoning_cap: crate::REASONING_CHAR_CAP,
        }
    }
}

pub struct LoopConfig {
    pub model: Model,
    pub effort: Effort,
    pub tool_execution: ExecutionMode,
    pub convert_to_llm: Box<ConvertFn>,
    /// Read per request and appended after the history, never to it; converted on its own.
    pub request_tail: Option<Box<TailFn>>,
    /// Whether a later request reads this session's tail (D295); the forced-none last word
    /// says `LastTurn` on its own.
    pub reuse: yi_types::model::Reuse,
    pub should_stop_after_turn: Option<Box<StopFn>>,
    pub prepare_next_turn: Option<Box<PrepareFn>>,
    pub get_steering_messages: Option<Box<QueueFn>>,
    pub get_follow_up_messages: Option<Box<QueueFn>>,
    /// Design §4.4: runs at every message boundary inside the tool loop, since a tool-heavy
    /// turn can blow the window mid-turn. Some(new) replaces the in-flight history.
    pub maybe_compact: Option<Box<CompactFn>>,
    pub first_turn_tool_choice: Option<ToolChoice>,
    /// Invariant: consulted synchronously, only when a turn ended with no tool calls and an
    /// empty steering queue. Some forces one more turn carrying the message.
    pub intercept_stop: Option<Box<InterceptFn>>,
    pub waiting: Option<std::sync::Arc<WaitingFn>>,
    /// Asked once when a stop ends a turn on tool calls: Some runs one last turn, tool-less.
    pub last_word: Option<Box<InterceptFn>>,
    /// Polled while a request streams: true aborts it, and the last word follows.
    pub last_word_due: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    /// Work started beside the request: awaited before each tool batch and before `AgentEnd`.
    pub side_work: Option<Box<GateFn>>,
    pub guards: LoopGuards,
}

impl LoopConfig {
    pub fn new(model: Model) -> Self {
        Self {
            effort: model.clamp_effort(Effort::default()),
            model,
            tool_execution: ExecutionMode::default(),
            convert_to_llm: Box::new(|messages| messages.to_vec()),
            request_tail: None,
            reuse: yi_types::model::Reuse::Loop,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
            maybe_compact: None,
            first_turn_tool_choice: None,
            intercept_stop: None,
            waiting: None,
            last_word: None,
            last_word_due: None,
            side_work: None,
            guards: LoopGuards::default(),
        }
    }
}
