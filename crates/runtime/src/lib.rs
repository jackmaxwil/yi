#![forbid(unsafe_code)]

pub mod provider;
pub mod session;
pub mod tools;

pub use provider::{ProviderStream, available_models, resolve_model};
pub use session::{AgentSession, SessionConfig, SessionError, Status};
pub use tools::ToolAdapter;
pub use yi_ai::auth;
pub use yi_ai::faux;
pub use yi_loop::ExecutionMode;
pub use yi_session as session_store;
pub use yi_tools::{builtin_tools, discover_exec_tools};
