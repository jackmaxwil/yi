//! The harness runs the check: `sh -c <command>` in the cwd, bounded by the spec's timeout,
//! its stdout parsed as TAP. A bash run by the model is not a run.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use yi_types::spec::{
    CHECK_TIMEOUT_FACTOR, CHECK_TIMEOUT_MAX, CHECK_TIMEOUT_MIN, CheckCommand, Exit, NotRun,
    Outcome, Run, RunKind,
};

pub const CAPTURE_CAP: usize = 262_144;
pub const TAIL_CHARS: usize = 2_000;

fn tail(capture: &yi_tools::CommandCapture) -> String {
    let mut combined = String::new();
    if !capture.stdout.trim().is_empty() {
        combined.push_str(capture.stdout.trim_end());
    }
    if !capture.stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(capture.stderr.trim_end());
    }
    let start = combined
        .char_indices()
        .rev()
        .nth(TAIL_CHARS.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    combined.get(start..).unwrap_or(&combined).to_owned()
}

/// The baseline's duration times the factor, within the bounds.
pub fn timeout_for(baseline: Duration) -> Duration {
    baseline
        .saturating_mul(CHECK_TIMEOUT_FACTOR)
        .clamp(CHECK_TIMEOUT_MIN, CHECK_TIMEOUT_MAX)
}

#[must_use]
pub fn run_check(
    check: &CheckCommand,
    cwd: &Path,
    timeout: Duration,
    kind: RunKind,
    turn: u32,
) -> Run {
    let started = Instant::now();
    let mut command = yi_tools::command("sh");
    command.arg("-c").arg(check.as_str()).current_dir(cwd);
    let deadline = started.checked_add(timeout).unwrap_or(started);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || Instant::now() >= deadline);
    let at = yi_session::now_ms();
    let outcome = match yi_tools::run_captured(command, None, &cancelled, CAPTURE_CAP) {
        Err(error) => (Outcome::NotRun(NotRun::Spawn { error }), String::new()),
        Ok(capture) if capture.cancelled => (
            Outcome::NotRun(NotRun::Timeout { after: timeout }),
            tail(&capture),
        ),
        Ok(capture) => {
            let parsed = super::tap::parse(&capture.stdout);
            let tail = tail(&capture);
            if !parsed.any {
                (Outcome::NotRun(NotRun::NoTap), tail)
            } else {
                let exit = capture.exit_code.map_or(Exit::Signal, Exit::Code);
                (
                    Outcome::Ran {
                        exit,
                        status: parsed.status,
                        diag: parsed.diag,
                        metrics: parsed.metrics,
                        unbound: parsed.unbound,
                    },
                    tail,
                )
            }
        }
    };
    Run {
        at,
        turn,
        kind,
        duration: started.elapsed(),
        tail: outcome.1,
        outcome: outcome.0,
        literal_overlaps: Vec::new(),
    }
}
