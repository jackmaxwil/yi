//! What the host hands one contained spawn besides its profile.

use std::sync::Arc;

/// `env` rides `Command::env`, never `argv`, which `ps` reads, so it may hold a secret; `assign` rides
/// `env`'s argv past the scrub, so it never does. `after` returns rows the model sees beside the output.
pub struct Spawn {
    pub env: Vec<(String, String)>,
    pub assign: Vec<String>,
    pub after: Box<dyn FnOnce() -> Vec<String> + Send>,
}

pub type SpawnFn = Arc<dyn Fn(&str) -> Result<Spawn, SpawnRefused> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnRefused {
    #[error("[not started: {why}; contained commands reach the network only through it]")]
    Egress { why: String },
}
