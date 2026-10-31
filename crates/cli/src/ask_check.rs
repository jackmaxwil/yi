use std::path::Path;
use std::time::Instant;

use yi_runtime::goal::{CheckFailure, DEFAULT_CHECK_TIMEOUT_MS};

use super::ask::{Outcome, Screen, stream};

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
    };
    let mut evidence = String::new();
    let release = loop {
        if run.code != 0 || run.kind != "stop" {
            break Release::Ended;
        }
        if spent(ends) {
            break Release::Deadline;
        }
        let failed = match check_all(checks, dir, ends).await {
            Ok(()) => break Release::Green,
            Err(failure) => failure,
        };
        tally.reds = tally.reds.saturating_add(1);
        match failed {
            CheckFailure::Red(output) => evidence = output,
            other => {
                evidence = other.evidence();
                break match other {
                    CheckFailure::TimedOut(_) => Release::Timeout,
                    CheckFailure::Spawn(_) => Release::Spawn,
                    _ => Release::Ended,
                };
            }
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
    };
    let code = release_code(release, run.code);
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

fn release_code(release: Release, run_code: i32) -> i32 {
    match release {
        Release::Green => run_code,
        Release::Ended if run_code != 0 => run_code,
        Release::Cap | Release::Deadline | Release::Ended | Release::Spawn | Release::Timeout => {
            EXIT_RED
        }
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
    let (reds, reprompts) = (tally.reds, tally.reprompts);
    let row = match release {
        Release::Green => {
            format!("[check `{named}` green after {reprompts} re-prompt(s); {reds} red run(s)]")
        }
        Release::Cap => format!(
            "[check `{named}` released red: {reds} red runs, {reprompts} re-prompts (cap {REPROMPTS})]"
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

    #[test]
    fn only_green_can_keep_a_success_exit() {
        assert_eq!(super::release_code(super::Release::Green, 0), 0);
        assert_eq!(
            super::release_code(super::Release::Deadline, 0),
            super::EXIT_RED
        );
    }
}
