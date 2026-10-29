use std::path::Path;
use std::time::Instant;

use yi_runtime::goal::{CheckFailure, DEFAULT_CHECK_TIMEOUT_MS};

use super::ask::{Outcome, Screen, stream};

/// The tools that can change what a check reads. A re-prompted run that called only readers
/// leaves the check red for the same reason, so its rerun is skipped and counted as wasted.
pub(super) const PROGRESS: [&str; 4] = ["write", "edit", "bash", "ipython"];

/// The re-prompts one run may spend before the check's red is final.
const REPROMPTS: u32 = 3;
const EXIT_RED: i32 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Release {
    Green,
    /// The 4th red run, after [`REPROMPTS`] re-prompts.
    Cap,
    /// No time left before the deadline reserve the run keeps for its shutdown.
    Deadline,
    /// The run itself ended on a deadline, an error, an abort or a refusal.
    Ended,
    Spawn,
    Timeout,
}

impl Release {
    fn name(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Cap => "cap",
            Self::Deadline => "deadline",
            Self::Ended => "ended",
            Self::Spawn => "spawn",
            Self::Timeout => "timeout",
        }
    }
}

struct Tally {
    reds: u32,
    reprompts: u32,
    wasted: u32,
}

/// Re-prompts the same session with a red check's evidence until it passes, the run ends, or
/// the cap is reached; every ending prints one row and one `check` line.
pub(super) async fn chase(
    screen: &Screen<'_>,
    checks: &[String],
    dir: &Path,
    first: Outcome,
) -> Outcome {
    let ends = screen.ends;
    let mut run = first;
    let mut tally = Tally {
        reds: 0,
        reprompts: 0,
        wasted: 0,
    };
    let mut evidence = String::new();
    let release = loop {
        let mut failed = None;
        if tally.reprompts == 0 || run.acted {
            if spent(ends) {
                break Release::Deadline;
            }
            match check_all(checks, dir, ends).await {
                Ok(()) => break Release::Green,
                Err(failure) => failed = Some(failure),
            }
        } else {
            tally.wasted = tally.wasted.saturating_add(1);
        }
        tally.reds = tally.reds.saturating_add(1);
        match failed {
            Some(CheckFailure::Red(output)) => evidence = output,
            Some(other) => {
                evidence = other.evidence();
                break match other {
                    CheckFailure::TimedOut(_) => Release::Timeout,
                    CheckFailure::Spawn(_) => Release::Spawn,
                    _ => Release::Ended,
                };
            }
            None => {}
        }
        if tally.reds > REPROMPTS {
            break Release::Cap;
        }
        if spent(ends) {
            break Release::Deadline;
        }
        if tally.reprompts == 0 {
            eprintln!(
                "[check `{}` is red; re-prompting the same session with its evidence, up to {REPROMPTS} times]",
                checks.join("`, `")
            );
        }
        tally.reprompts = tally.reprompts.saturating_add(1);
        let prompt = yi_runtime::session::user_input(&reprompt(&evidence));
        if screen.session.prompt_message(prompt).is_err() {
            break Release::Ended;
        }
        run = stream(screen).await;
        if run.code != 0 || run.kind != "stop" {
            break Release::Ended;
        }
    };
    let code = if tally.reds == 0 || release == Release::Green {
        run.code
    } else {
        EXIT_RED
    };
    report(
        release,
        checks,
        &tally,
        &evidence,
        run.kind.as_str(),
        screen.modes.json,
    );
    Outcome {
        code,
        line: run.line,
        ..run
    }
}

fn report(
    release: Release,
    checks: &[String],
    tally: &Tally,
    evidence: &str,
    ended: &str,
    json: bool,
) {
    let named = checks.join("`, `");
    let (reds, reprompts, wasted) = (tally.reds, tally.reprompts, tally.wasted);
    let row = match release {
        Release::Green => {
            format!("[check `{named}` green after {reprompts} re-prompt(s); {reds} red run(s)]")
        }
        Release::Cap => format!(
            "[check `{named}` released red: {reds} red runs, {reprompts} re-prompts (cap {REPROMPTS}); {wasted} re-prompt(s) changed nothing]"
        ),
        Release::Deadline => format!(
            "[check `{named}` released red: no time left before the deadline reserve; {reds} red run(s), {reprompts} re-prompt(s)]"
        ),
        Release::Ended => format!(
            "[check `{named}` released red: the run ended on `{ended}`; {reds} red run(s), {reprompts} re-prompt(s)]"
        ),
        Release::Spawn => format!(
            "[check `{named}` released red: it could not spawn; {}]",
            first_line(evidence)
        ),
        Release::Timeout => format!(
            "[check `{named}` released red: it timed out; {}]",
            first_line(evidence)
        ),
    };
    eprintln!("{row}");
    if json {
        let line = serde_json::json!({
            "type": "check",
            "kind": release.name(),
            "commands": checks,
            "reds": reds,
            "reprompts": reprompts,
            "wasted": wasted,
            "evidence": first_line(evidence),
        });
        println!("{line}");
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn reprompt(evidence: &str) -> String {
    format!(
        "a check run outside the session after your last turn did not pass:\n{evidence}\nFix what it names, make it pass, then stop."
    )
}

/// Every check in turn; the first that does not pass is the one the evidence names.
async fn check_all(
    checks: &[String],
    dir: &Path,
    ends: Option<Instant>,
) -> Result<(), CheckFailure> {
    for check in checks {
        let (check, dir) = (check.clone(), dir.to_path_buf());
        let budget = budget_ms(ends);
        let done = tokio::task::spawn_blocking(move || {
            yi_runtime::goal::run_check_typed(&check, &dir, budget)
        })
        .await
        .map_err(|join| CheckFailure::Spawn(join.to_string()))?;
        done?;
    }
    Ok(())
}

/// The check's own cap, cut to the time left before the run keeps its shutdown reserve.
fn budget_ms(ends: Option<Instant>) -> u64 {
    let left = match ends {
        Some(at) => at.saturating_duration_since(Instant::now()).as_millis(),
        None => return DEFAULT_CHECK_TIMEOUT_MS,
    };
    u64::try_from(left)
        .unwrap_or(DEFAULT_CHECK_TIMEOUT_MS)
        .min(DEFAULT_CHECK_TIMEOUT_MS)
}

fn spent(ends: Option<Instant>) -> bool {
    ends.is_some_and(|at| Instant::now() >= at)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    /// Dies with the check's own cap unread: a run with 20 s of deadline left would let one
    /// check hold the process for its full ten minutes.
    #[test]
    fn a_check_never_gets_past_the_deadline_reserve() {
        let far = Instant::now().checked_add(Duration::from_secs(3_600));
        assert_eq!(super::budget_ms(far), super::DEFAULT_CHECK_TIMEOUT_MS);
        let near = Instant::now().checked_add(Duration::from_secs(4));
        let budget = super::budget_ms(near);
        assert!(budget <= 4_000, "{budget}");
        assert!(!super::spent(far));
        assert!(super::spent(
            Instant::now().checked_sub(Duration::from_secs(1))
        ));
        assert_eq!(super::budget_ms(None), super::DEFAULT_CHECK_TIMEOUT_MS);
    }

    /// Dies with a run's read-only turn counted as progress: the rerun it cannot change is
    /// spent again, and the model is re-prompted for nothing.
    #[test]
    fn only_a_writing_or_exec_tool_counts_as_progress() {
        assert!(super::PROGRESS.contains(&"bash"));
        assert!(super::PROGRESS.contains(&"write"));
        assert!(!super::PROGRESS.contains(&"read"));
        assert!(!super::PROGRESS.contains(&"get_context"));
    }
}
