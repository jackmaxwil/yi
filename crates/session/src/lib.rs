#![forbid(unsafe_code)]

mod error;
mod id;
mod jsonl;
mod query;
mod repo;
mod state;
mod store;

pub use error::SessionError;
pub use id::{IdGenerator, now_ms, validate_session_id};
pub use jsonl::{JsonlRepo, create_flat_session, load_session};
pub use query::{
    BranchBounds, CreateOptions, EntryOrder, EntryQuery, ForkPosition, ForkScope, LanePointer,
    LogOptions, RecordQuery, SessionMetadata, SessionStats,
};
pub use repo::{MemRepo, SessionRepo, SharedSession, lock_session};
pub use store::SessionStore;
