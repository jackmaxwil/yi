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
    let deadline = input
        .get("wait")
        .and_then(Value::as_u64)
        .map(super::clamp_wait);
    if input.get("kill").and_then(Value::as_bool) == Some(true) {
        if let Err(error) = jobs.kill(id) {
            return error_output(error.to_string());
        }
        jobs.wait_settled(Some(id), deadline.unwrap_or_else(|| Duration::from_secs(5)));
    }
    jobs.set_reported(id, true);
    let started = std::time::Instant::now();
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
        let viewed = crate::reduce::view(
            crate::reduce::Involuntary::Job(&report.command),
            &chunk.text,
            "",
            -1,
            None,
            context.recovery_dir.as_deref(),
        );
        sections.push(viewed.text);
    }
    let mut output = text_output(sections.join("\n"));
    output.result.details = json!({ "job": id.0, "running": true });
    output.value = context.program.then(|| crate::values::job(id));
    output
}
