use yi_types::message::AgentMessage;
use yi_types::model::{Effort, Model};

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
    /// Design P13: runs at every message boundary inside the tool loop — a
    /// tool-heavy turn can blow the window before the turn ends. Some(new)
    /// replaces the in-flight history with the compacted view.
    pub maybe_compact: Option<Box<CompactFn>>,
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
        }
    }
}
