use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{JobId, JobReport, JobState, Jobs, Outcome};
use crate::tool::{ToolContext, ToolOutput, error_output, text_output};

pub(crate) fn poll_job(input: &Map<String, Value>, context: &ToolContext) -> ToolOutput {
    let jobs = super::registry();
    let requested = input.get("job").and_then(Value::as_u64).map(JobId);
    let id = match requested {
        Some(id) if jobs.report_for(id, context.job_owner).is_some() => id,
        Some(_) => return error_output("no background job to check on"),
        None => {
            let Some(report) = jobs.latest(context.job_owner) else {
                return error_output("no background job to check on");
            };
            report.id
        }
    };
    let requested_wait = input
        .get("wait")
        .and_then(Value::as_u64)
        .map(super::clamp_wait);
    let kill = input.get("kill").and_then(Value::as_bool) == Some(true);
    let deadline = match (requested_wait, kill) {
        (None, true) => Some(Duration::from_secs(5)),
        (deadline, _) => deadline,
    };
    let started = std::time::Instant::now();
    if kill && let Err(error) = jobs.kill(id) {
        return error_output(error.to_string());
    }
    jobs.set_reported(id, true);
    let _span = yi_types::trace::span("wait.job");
    loop {
        let Some(report) = jobs.report_for(id, context.job_owner) else {
            return error_output("no background job to check on");
        };
        if report.finished {
            jobs.mark_delivered(id);
            let exit_code = report.exit_code.unwrap_or(-1);
            let text = format!("{}\n{}", report.headline(), report.view());
            let killed = report.state == JobState::Settled(Outcome::Killed);
            let details = json!({ "job": id.0, "exitCode": exit_code });
            let value = context
                .program
                .then(|| crate::values::polled(id, report.exit_code, &report.output, killed));
            let mut output = crate::tool::ran_output(text, details, killed);
            output.value = value;
            return output;
        }
        match deadline {
            Some(limit) if started.elapsed() < limit && !(context.cancelled)() => {
                let left = limit.saturating_sub(started.elapsed());
                jobs.wait_settled(Some(id), left.min(Duration::from_secs(1)));
            }
            _ => {
                jobs.set_reported(id, false);
                return running(jobs, id, &report, context);
            }
        }
    }
}

fn running(jobs: &Jobs, id: JobId, report: &JobReport, context: &ToolContext) -> ToolOutput {
    let mut sections = vec![report.headline()];
    if let Ok(chunk) = jobs.output_since(id, 0)
        && !chunk.text.is_empty()
    {
        let total = usize::try_from(chunk.next).unwrap_or(usize::MAX);
        if chunk.dropped > 0 {
            let kept =
                usize::try_from(chunk.next.saturating_sub(chunk.dropped)).unwrap_or(usize::MAX);
            let next = crate::reduce::rerun("bash", &report.command, "view.log");
            sections.push(crate::tool::cut_row(
                kept,
                total,
                "bytes of live output kept",
                "OUTPUT_CAP",
                crate::process::OUTPUT_CAP,
                &next,
            ));
        }
        let viewed = crate::reduce::view_running(
            &report.command,
            &chunk.text,
            total,
            context.recovery_dir.as_deref(),
        );
        sections.push(viewed.text);
    }
    let mut output = text_output(sections.join("\n"));
    output.result.details = json!({ "job": id.0, "running": true });
    output.value = context.program.then(|| crate::values::job(id));
    output
}
#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::super::{JobOwner, Reaper, registry};
    use crate::Tool;

    #[test]
    fn kill_and_poll_share_one_deadline() -> Result<(), Box<dyn Error>> {
        let cwd = Path::new("/yi-jobs-test/kill-deadline");
        let owner = JobOwner::mint();
        let killed = Arc::new(AtomicBool::new(false));
        let id = registry().insert(
            "sleep 30",
            (cwd, None),
            Reaper::Poller {
                reported: false,
                owner: Some(owner),
            },
            Arc::default(),
            Arc::clone(&killed),
        );
        let input = serde_json::Map::from_iter([
            ("job".to_owned(), serde_json::json!(id.0)),
            ("kill".to_owned(), serde_json::json!(true)),
            ("wait".to_owned(), serde_json::json!(5)),
        ]);
        let mut context = crate::ToolContext::new(cwd.to_path_buf());
        context.job_owner = Some(owner);
        let started = Instant::now();
        let output = crate::BashTool::default().execute(input, &context);
        let elapsed = started.elapsed();
        assert_eq!(output.result.details["running"], true, "{output:?}");
        assert!(
            killed.load(Ordering::SeqCst),
            "kill was ignored while the poll waited"
        );
        assert!(
            elapsed < Duration::from_secs(7),
            "returned after {elapsed:?}"
        );
        Ok(())
    }
}
