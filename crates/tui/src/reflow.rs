use std::time::{Duration, Instant};

/// U35 (codex `transcript_reflow.rs:18`). Trailing, so dragging a terminal edge
/// rebuilds scrollback once at the settled width instead of at every
/// intermediate one.
pub const REFLOW_DEBOUNCE: Duration = Duration::from_millis(75);

/// Per-terminal row caps for a rebuild (U36, codex `resize_reflow_cap.rs`).
/// These mirror documented scrollback defaults: replaying more rows than the
/// terminal retains is work nobody can scroll back to see.
const VSCODE_MAX_ROWS: usize = 1_000;
const WINDOWS_TERMINAL_MAX_ROWS: usize = 9_001;
const WEZTERM_MAX_ROWS: usize = 3_500;
const ALACRITTY_MAX_ROWS: usize = 10_000;
const FALLBACK_MAX_ROWS: usize = 1_000;

/// codex reads this from `codex_terminal_detection`; that crate is not a §13.3
/// dependency Yi will take for four constants, so the same two environment
/// variables are read directly.
pub fn reflow_max_rows() -> usize {
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    if std::env::var_os("VSCODE_INJECTION").is_some() || program == "vscode" {
        return VSCODE_MAX_ROWS;
    }
    if std::env::var_os("WT_SESSION").is_some() {
        return WINDOWS_TERMINAL_MAX_ROWS;
    }
    if program == "WezTerm" {
        return WEZTERM_MAX_ROWS;
    }
    if program == "alacritty" || term.contains("alacritty") {
        return ALACRITTY_MAX_ROWS;
    }
    FALLBACK_MAX_ROWS
}

/// How the latest draw width relates to the previous observed one.
pub struct WidthChange {
    pub changed: bool,
    pub initialized: bool,
}

/// U35: pending transcript-scrollback repair after a resize.
///
/// Observed width and rebuilt width are deliberately separate. A terminal can
/// report an intermediate size during a drag and settle on the final one after
/// the rebuild already ran, so the next draw has to be able to ask for one more
/// rebuild even though the observed-width tracker has seen that value.
#[derive(Debug, Default)]
pub struct ReflowState {
    last_observed_width: Option<u16>,
    last_reflow_width: Option<u16>,
    pending_reflow_width: Option<u16>,
    pending_until: Option<Instant>,
    ran_during_stream: bool,
    resize_requested_during_stream: bool,
}

impl ReflowState {
    /// Record the width seen during a draw. The first one initializes without
    /// scheduling: no old-width transcript has been emitted yet, so treating
    /// initialization as a resize would make the first draw rebuild for nothing.
    pub fn note_width(&mut self, width: u16) -> WidthChange {
        let previous = self.last_observed_width.replace(width);
        if previous.is_none() {
            self.last_reflow_width = Some(width);
        }
        WidthChange {
            changed: previous.is_some_and(|previous| previous != width),
            initialized: previous.is_none(),
        }
    }

    /// Whether scrollback still needs rebuilding at `width`, compared against
    /// the width that actually rebuilt it rather than the last one observed.
    pub fn reflow_needed_for_width(&self, width: u16) -> bool {
        self.last_reflow_width != Some(width) && self.pending_reflow_width != Some(width)
    }

    /// Arm the trailing debounce. Later events push the deadline out.
    pub fn schedule_debounced(&mut self, target_width: Option<u16>, now: Instant) {
        if let Some(target_width) = target_width {
            self.pending_reflow_width = Some(target_width);
        }
        self.pending_until = Some(now + REFLOW_DEBOUNCE);
    }

    /// Run at the next opportunity. Used after a stream consolidates, where
    /// waiting out the debounce would leave terminal-wrapped stream rows in the
    /// finalized transcript.
    pub fn schedule_immediate(&mut self, now: Instant) {
        self.pending_reflow_width = None;
        self.pending_until = Some(now);
    }

    pub fn pending_until(&self) -> Option<Instant> {
        self.pending_until
    }

    pub fn pending_is_due(&self, now: Instant) -> bool {
        self.pending_until.is_some_and(|deadline| now >= deadline)
    }

    pub fn clear_pending_reflow(&mut self) {
        self.pending_until = None;
        self.pending_reflow_width = None;
    }

    /// Remember the width that actually rebuilt scrollback — "seen during a
    /// draw" is not "repaired at this width".
    pub fn mark_reflowed_width(&mut self, width: u16) -> bool {
        self.last_reflow_width.replace(width) != Some(width)
    }

    /// A mid-stream rebuild can only render the partial that existed then, so
    /// the end of the stream has to rebuild once more from the settled text.
    pub fn mark_ran_during_stream(&mut self) {
        self.ran_during_stream = true;
    }

    /// The width changed while a stream was live and the debounce had not yet
    /// fired; without this the stream could finish with no final repair.
    pub fn mark_resize_requested_during_stream(&mut self) {
        self.resize_requested_during_stream = true;
    }

    /// Draining read: each resize-during-stream episode forces at most one
    /// post-stream repair.
    pub fn take_stream_finish_needed(&mut self) -> bool {
        let needed = self.ran_during_stream || self.resize_requested_during_stream;
        self.ran_during_stream = false;
        self.resize_requested_during_stream = false;
        needed
    }
}
