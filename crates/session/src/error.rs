use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Already exists: {0}")]
    AlreadyExists(String),
    #[error("Invalid entry: {0}")]
    InvalidEntry(String),
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),
    #[error("Lane not found: {0}")]
    InvalidLane(String),
    #[error("Invalid query: {0}")]
    InvalidQuery(String),
    #[error("Invalid fork target: {0}")]
    InvalidForkTarget(String),
    #[error("Storage error: {0}")]
    Storage(String),
}

impl SessionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::AlreadyExists(_) => "already_exists",
            Self::InvalidEntry(_) => "invalid_entry",
            Self::InvalidPayload(_) => "invalid_payload",
            Self::InvalidLane(_) => "invalid_lane",
            Self::InvalidQuery(_) => "invalid_query",
            Self::InvalidForkTarget(_) => "invalid_fork_target",
            Self::Storage(_) => "storage",
        }
    }
}
