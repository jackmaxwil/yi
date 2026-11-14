pub enum Command {
    Prompt(String),
    Steer(String),
    PromptImages(String, Vec<crate::clipboard::ClipImage>),
    SteerImages(String, Vec<crate::clipboard::ClipImage>),
    /// E1: the rewind itself is synchronous on the render thread; the
    /// summarizer call it earns is not.
    SummarizeBranch(yi_runtime::BranchStub),
    Slash(String),
    StopChild(String),
    ChildHistory(String),
    Answer {
        child_id: String,
        question: String,
        text: String,
    },
    Abort,
    Shutdown,
}
