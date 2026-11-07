/// A check's command, its proxy lease, and what kills what a killed `docker exec` left running.
pub(crate) struct CheckShell {
    pub(crate) command: std::process::Command,
    pub(super) lease: Option<crate::egress::Lease>,
    pub(super) sweep: Option<String>,
}

/// Runs a check to its capture, holding its lease until then. Invariant: a check past its deadline
/// is swept before it reads as settled, or what it started goes on writing the tree.
pub(crate) fn captured(
    shell: CheckShell,
    stdin: Option<Vec<u8>>,
    cancelled: &yi_tools::CancelFlag,
) -> Result<yi_tools::CommandCapture, String> {
    let mut capture =
        yi_tools::run_captured(shell.command, stdin, cancelled, yi_tools::OUTPUT_CAP)?;
    capture.rows = shell
        .lease
        .map(crate::egress::Lease::revoke)
        .unwrap_or_default();
    if let (true, Some(sweep)) = (capture.cancelled, &shell.sweep) {
        match yi_tools::jobs::run_sweep_within(sweep, Some(yi_tools::jobs::SWEEP_BOUND)) {
            yi_tools::jobs::SweepOutcome::Completed => {}
            yi_tools::jobs::SweepOutcome::TimedOut => capture.rows.push(
                "[… the container sweep did not finish inside SWEEP_BOUND (5 s); what a killed call left may still write the tree]"
                    .to_owned(),
            ),
            yi_tools::jobs::SweepOutcome::Failed(reason) => capture.rows.push(format!(
                "[… the container sweep {reason}; what a killed call left may still write the tree]"
            )),
        }
    }
    Ok(capture)
}
