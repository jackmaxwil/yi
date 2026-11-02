//! What the host hands one contained spawn besides its profile.

use std::sync::Arc;

/// Reaches the child through `Command::env`, never `argv`, which `ps` reads. `after` runs when the
/// child is done, and what it returns are rows the model sees beside the output.
pub struct Spawn {
    pub env: Vec<(String, String)>,
    pub after: Box<dyn FnOnce() -> Vec<String> + Send>,
}

pub type SpawnFn = Arc<dyn Fn() -> Result<Spawn, SpawnRefused> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnRefused {
    #[error("[not started: {why}; contained commands reach the network only through it]")]
    Egress { why: String },
}
