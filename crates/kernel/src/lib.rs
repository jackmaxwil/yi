#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod client;
pub mod connection;
pub mod framing;
pub mod journal;
pub(crate) mod pump;
pub mod reduce;

// Generous backstop for a kernel that is alive but wedged: crashes are detected
// within one 25ms poll via the exit handler, warm boots return in under a second,
// and a cold first boot after a venv (re)provision may legitimately need tens of
// seconds of imports before it binds ports and answers the ready probe.
pub const PORTS_RESOLVE_TIMEOUT_MS: u64 = 30_000;
pub const READY_TIMEOUT_MS: u64 = 30_000;
// Loopback PUB/SUB subscription propagation is usually sub-ms, but keep a small guard before first execute.
pub const IOPUB_SUBSCRIBE_DELAY_MS: u64 = 50;
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
pub const KERNEL_ABORT_GRACE_MS: u64 = 1_000;
pub const KERNEL_BUSY_REUSE_WAIT_MS: u64 = 5_000;
pub const KERNEL_BUSY_INTERRUPT_INTERVAL_MS: u64 = 500;
pub const MAX_LATE_SENT_AGENT_MESSAGE_HANDLERS: usize = 256;
pub const KERNEL_BUSY_AFTER_INTERRUPT_MESSAGE: &str = "IPython kernel is still running the previously interrupted cell. Wait and try again, or kill the IPython kernel to start fresh.";

// Hard ceiling on a single attachment's base64 payload, a defensive guard
// against a runaway direct `display_data` emit. The `attach-image` skill caps
// its own images well under this, so a skill-produced attachment is never
// dropped here — only a non-skill emit can hit this.
pub const MAX_ATTACHMENT_DATA_CHARS: usize = 10_000_000;

/// Comm target the kernel-side `rlm.host_request` shim opens for typed host requests.
pub const HOST_COMM_TARGET: &str = "host.request";

pub const DIFF_DISPLAY_MIME: &str = "application/vnd.prime-agent.diff+json";
pub const ATTACHMENT_DISPLAY_MIME: &str = "application/vnd.prime-agent.attachment+json";
pub const AGENT_MESSAGE_DISPLAY_MIME: &str = "application/vnd.prime-agent.agent-message+json";
