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
type TransformFn = dyn Fn(&[AgentMessage]) -> Option<Vec<AgentMessage>> + Send + Sync;
type StopFn = dyn Fn(&TurnSnapshot) -> bool + Send + Sync;
type PrepareFn = dyn Fn(&TurnSnapshot) -> Option<NextTurn> + Send + Sync;
type QueueFn = dyn Fn() -> Vec<AgentMessage> + Send + Sync;
type InterceptFn = dyn Fn(&TurnSnapshot) -> Option<AgentMessage> + Send + Sync;
type CompactFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<AgentMessage>>> + Send>>;
type CompactFn = dyn Fn(&[AgentMessage]) -> CompactFuture + Send + Sync;

pub struct LoopConfig {
    pub model: Model,
    pub effort: Effort,
    pub tool_execution: ExecutionMode,
    pub convert_to_llm: Box<ConvertFn>,
    pub transform_context: Option<Box<TransformFn>>,
    pub should_stop_after_turn: Option<Box<StopFn>>,
    pub prepare_next_turn: Option<Box<PrepareFn>>,
    pub get_steering_messages: Option<Box<QueueFn>>,
    pub get_follow_up_messages: Option<Box<QueueFn>>,
    /// Design P13: runs at every message boundary inside the tool loop, since a tool-heavy
    /// turn can blow the window mid-turn. Some(new) replaces the in-flight history.
    pub maybe_compact: Option<Box<CompactFn>>,
    pub first_turn_tool_choice: Option<ToolChoice>,
    /// Invariant: consulted synchronously, only when a turn ended with no tool calls and an
    /// empty steering queue. Some forces one more turn carrying the message.
    pub intercept_stop: Option<Box<InterceptFn>>,
}

impl LoopConfig {
    pub fn new(model: Model) -> Self {
        Self {
            effort: model.clamp_effort(Effort::default()),
            model,
            tool_execution: ExecutionMode::default(),
            convert_to_llm: Box::new(|messages| messages.to_vec()),
            transform_context: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
            maybe_compact: None,
            first_turn_tool_choice: None,
            intercept_stop: None,
        }
    }
}
