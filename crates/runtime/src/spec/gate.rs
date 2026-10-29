//! The stop rule: a clean stop is accepted only when a frozen spec exists, its last harness
//! run is after the last write, every requirement is green and the refute passed.

use std::time::Duration;

use yi_types::spec::{RunKind, Spec};

use super::{SpecStore, ids_text};

/// The reason a stop was refused; its record string is the `todo_intercept` reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    Missing,
    Unfrozen,
    CheckBroken,
    Red,
    Refute,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "spec_missing",
            Self::Unfrozen => "spec_unfrozen",
            Self::CheckBroken => "spec_check_broken",
            Self::Red => "spec_red",
            Self::Refute => "spec_refute",
        }
    }
}

/// Without a deadline a reason refuses this many stops per prompt, then passes with a record:
/// an infinite refusal with no clock is a hang, not a rule.
pub const REFUSAL_CAP: u8 = 3;
pub const GREEN_STOP: &str = "spec_green_stop";
pub const WAIVED: &str = "spec_stop_waived";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Refuse { reason: Reason, text: String },
}

fn clock(left: Option<Duration>) -> String {
    left.map(|left| format!(" {} min left.", left.as_secs() / 60))
        .unwrap_or_default()
}

fn refuse(reason: Reason, text: String) -> Verdict {
    Verdict::Refuse { reason, text }
}

/// The facts of this stop, in the order a reader needs them.
#[must_use]
pub fn judge(store: &SpecStore, left: Option<Duration>) -> Verdict {
    // A child's stop is judged by its parent through its slice (D171), never here.
    if !store.is_root() {
        return Verdict::Pass;
    }
    let Some(spec) = store.spec() else {
        let expectation = store.expectation();
        if !expectation.expected {
            return Verdict::Pass;
        }
        return refuse(
            Reason::Missing,
            format!(
                "No spec exists. Write `check.py` printing one TAP line per requirement (`ok N - R1 …` / `not ok N - R1 …`), then `spec freeze` with the requirements, the artifact paths and the command; the request enumerates {} items.{}",
                expectation.enumerated,
                clock(left)
            ),
        );
    };
    let frozen = match spec {
        Spec::Draft(_) => {
            return refuse(
                Reason::Unfrozen,
                format!(
                    "The spec is not frozen: fix the facts of the last refusal and `spec freeze` again.{}",
                    clock(left)
                ),
            );
        }
        Spec::Frozen(frozen) => frozen,
    };
    let command = frozen.draft.check.as_str().to_owned();
    let stale = store.writes_since_run() > 0
        || store
            .last_run()
            .is_none_or(|run| run.at < frozen.frozen_at || matches!(run.kind, RunKind::Refute(_)));
    let run = if stale {
        match store.run(RunKind::Run) {
            Ok(run) => run,
            Err(error) => {
                return refuse(
                    Reason::CheckBroken,
                    format!("The check could not run: {error}.{}", clock(left)),
                );
            }
        }
    } else {
        match store.last_run() {
            Some(run) => run,
            None => {
                return refuse(
                    Reason::CheckBroken,
                    format!("The check has not run: `spec run`.{}", clock(left)),
                );
            }
        }
    };
    if !run.ran() {
        return refuse(
            Reason::CheckBroken,
            format!(
                "The check did not run: `{command}` printed no TAP line or did not start. Fix it and `spec run`.\n{}{}",
                run.tail.trim_end(),
                clock(left)
            ),
        );
    }
    let Some(summary) = store.summary() else {
        return Verdict::Pass;
    };
    if !summary.red.is_empty() || !summary.missing.is_empty() {
        let mut lines = Vec::new();
        for id in &summary.red {
            let diag = run.diag(id).map(|d| format!(" ({d})")).unwrap_or_default();
            lines.push(format!("{id}{diag}"));
        }
        let mut text = format!(
            "`{command}`: {} of {} red",
            summary.red.len(),
            summary.total
        );
        if !lines.is_empty() {
            text.push_str(&format!(": {}", lines.join(", ")));
        }
        if !summary.missing.is_empty() {
            text.push_str(&format!(
                "; no TAP line names {}",
                ids_text(&summary.missing)
            ));
        }
        let ago = yi_session::now_ms().saturating_sub(run.at) / 1000;
        text.push_str(&format!(
            "; ran {ago}s ago, after your last write.{} Fix them and `spec run`; a clean stop is accepted only when every requirement is green.",
            clock(left)
        ));
        if !run.literal_overlaps.is_empty() {
            text.push_str(&format!(
                "\nTyped values: {}; expected values are computed from the inputs, never typed.",
                run.literal_overlaps.join(", ")
            ));
        }
        return refuse(Reason::Red, text);
    }
    match store.ensure_refute(&frozen) {
        Err(error) => refuse(
            Reason::Refute,
            format!(
                "The refute could not run: {error}. The artifacts may be aside under {}; check them before stopping.{}",
                store.dir().join("aside").display(),
                clock(left)
            ),
        ),
        Ok(record) if record.passed() => Verdict::Pass,
        Ok(record) => {
            let paths: Vec<String> = frozen
                .draft
                .artifacts
                .iter()
                .map(|artifact| format!("`{}`", artifact.path().display()))
                .collect();
            let paths = paths.join(", ");
            let mut parts = Vec::new();
            if !record.baseline_green.is_empty() {
                parts.push(format!(
                    "{} stay green with {paths} restored to the untouched tree",
                    ids_text(&record.baseline_green)
                ));
            }
            if !record.empty_green.is_empty() {
                parts.push(format!(
                    "{} stay green with {paths} emptied",
                    ids_text(&record.empty_green)
                ));
            }
            refuse(
                Reason::Refute,
                format!(
                    "{}; the check does not read the artifact it should. Make each assertion read the file the task names, then `spec run`.{}",
                    parts.join("; "),
                    clock(left)
                ),
            )
        }
    }
}
