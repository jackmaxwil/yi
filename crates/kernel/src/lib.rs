#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod client;
pub mod connection;
pub mod framing;
pub mod journal;
mod lock;
mod probe;
pub(crate) mod pump;
pub mod reduce;
#[cfg(test)]
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
pub mod snapshot;
pub mod uv_install;

// Generous backstop for a kernel alive but wedged: crashes surface in one 25ms poll and warm
// boots return in under a second, but a cold boot may need tens of seconds of imports.
pub const PORTS_RESOLVE_TIMEOUT_MS: u64 = 30_000;
pub const READY_TIMEOUT_MS: u64 = 30_000;
// Readiness resends kernel_info at most this far apart until iopub carries a message (slow joiner).
pub const IOPUB_PROBE_RESEND_MS: u64 = 50;
pub const DEFAULT_MAX_OUTPUT_CHARS: usize = 65_536;
pub const HOST_REQUEST_DISPOSE_TIMEOUT_MS: u64 = 5_000;
pub const KERNEL_SHUTDOWN_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_SNAPSHOT_DEBOUNCE_MS: u64 = 1_500;
// Snapshot/restore cells can be large to (de)serialize; give them room beyond the user cap.
pub const SNAPSHOT_MAX_OUTPUT_CHARS: usize = 1_000_000;
// Cap how long a graceful dispose waits on the final snapshot; the debounced
// on-disk copy is the fallback if this is exceeded.
pub const SNAPSHOT_DISPOSE_TIMEOUT_MS: u64 = 5_000;
pub const SNAPSHOT_EXECUTION_TIMEOUT_MS: u64 = 5_000;
pub const KERNEL_STATE_LISTING_TIMEOUT_MS: u64 = 5_000;
pub const KERNEL_ABORT_GRACE_MS: u64 = 1_000;
pub const KERNEL_BUSY_REUSE_WAIT_MS: u64 = 5_000;
pub const KERNEL_BUSY_INTERRUPT_INTERVAL_MS: u64 = 500;
pub const MAX_LATE_SENT_AGENT_MESSAGE_HANDLERS: usize = 256;
pub const KERNEL_BUSY_AFTER_INTERRUPT_MESSAGE: &str = "IPython kernel is still running the previously interrupted cell. Wait and try again, or kill the IPython kernel to start fresh.";

// Guards against a runaway direct `display_data` emit. `attach-image` caps its
// own images well under this, so only a non-skill emit can reach the ceiling.
pub const MAX_ATTACHMENT_DATA_CHARS: usize = 10_000_000;

/// Comm target the kernel-side `rlm.host_request` shim opens for typed host requests.
pub const HOST_COMM_TARGET: &str = "host.request";

pub const DIFF_DISPLAY_MIME: &str = "application/vnd.yi.diff+json";
pub const ATTACHMENT_DISPLAY_MIME: &str = "application/vnd.yi.attachment+json";
pub const AGENT_MESSAGE_DISPLAY_MIME: &str = "application/vnd.yi.agent-message+json";
