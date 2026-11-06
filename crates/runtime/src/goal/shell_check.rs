use std::sync::Arc;
use std::time::Instant;

use super::{CHECKER_ENV_BASE, Checker, captured, lease_env, probe};

/// Who a check's child sees: nothing of the host's, or the names its manifest declares.
pub(crate) enum Env<'a> {
    Keyless,
    Allowlist(&'a [String]),
}

/// One shell check. The goal's, plan verify's and the `exec://` adapter's runners are this
/// builder with a different environment policy, deadline and reading of the output.
pub(crate) struct ShellCheck<'a> {
    command: &'a str,
    cwd: &'a std::path::Path,
    checker: &'a Checker,
    env: Env<'a>,
    stdin: Option<Vec<u8>>,
    deadline: Option<Instant>,
    stop: Option<&'a yi_tools::CancelFlag>,
    probe: bool,
}

impl<'a> ShellCheck<'a> {
    pub(crate) fn new(command: &'a str, cwd: &'a std::path::Path, checker: &'a Checker) -> Self {
        Self {
            command,
            cwd,
            checker,
            env: Env::Keyless,
            stdin: None,
            deadline: None,
            stop: None,
            probe: false,
        }
    }

    pub(crate) fn env(mut self, env: Env<'a>) -> Self {
        self.env = env;
        self
    }

    pub(crate) fn stdin(mut self, stdin: Option<Vec<u8>>) -> Self {
        self.stdin = stdin;
        self
    }

    pub(crate) fn within(mut self, timeout_ms: u64) -> Self {
        self.deadline = Some(
            Instant::now()
                .checked_add(std::time::Duration::from_millis(timeout_ms))
                .unwrap_or_else(Instant::now),
        );
        self
    }

    pub(crate) fn until(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub(crate) fn stop(mut self, stop: Option<&'a yi_tools::CancelFlag>) -> Self {
        self.stop = stop;
        self
    }

    pub(crate) fn probe(mut self) -> Self {
        self.probe = true;
        self
    }

    /// # Errors
    /// A hook or probe refused it, or the shell did not spawn.
    pub(crate) fn run(self) -> Result<yi_tools::CommandCapture, String> {
        self.checker.guard(self.command, self.cwd)?;
        // Invariant: a check placed in a container is judged there, so its first word is not on the
        // host's PATH and the host probe would refuse a program the image has.
        if self.probe
            && self.checker.wall.container.is_none()
            && let Some(refusal) = probe(self.command, self.cwd)
        {
            return Err(refusal);
        }
        let program = match &self.env {
            Env::Keyless => "sh",
            Env::Allowlist(_) => "/bin/sh",
        };
        let mut shell = self.checker.shell(program, self.command, self.cwd);
        if let Env::Allowlist(declared) = self.env {
            shell.command.env_clear();
            let proxy: Vec<(String, String)> = lease_env(shell.lease.as_ref()).collect();
            shell.command.envs(proxy);
            let names = CHECKER_ENV_BASE
                .iter()
                .map(|name| (*name).to_owned())
                .chain(declared.iter().cloned());
            for name in names {
                if let Some(value) = std::env::var_os(&name) {
                    shell.command.env(name, value);
                }
            }
        }
        let deadline = self.deadline;
        let stop = self.stop.cloned();
        let cancelled: yi_tools::CancelFlag = Arc::new(move || {
            deadline.is_some_and(|deadline| Instant::now() >= deadline)
                || stop.as_ref().is_some_and(|stop| stop())
        });
        captured(shell, self.stdin, &cancelled)
    }
}

/// `/bin/sh -c`, `env_clear()` plus the allowlist; the caller reads the capture, and its Err is
/// no spawn or a refused Bash hook.
pub(crate) fn run_check_in(
    cwd: &std::path::Path,
    command: &str,
    env_names: &[String],
    stdin: Option<Vec<u8>>,
    deadline: Instant,
    stop: Option<&yi_tools::CancelFlag>,
    checker: &Checker,
) -> Result<yi_tools::CommandCapture, String> {
    ShellCheck::new(command, cwd, checker)
        .env(Env::Allowlist(env_names))
        .stdin(stdin)
        .until(deadline)
        .stop(stop)
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_without_a_deadline_is_not_cancelled() -> Result<(), Box<dyn std::error::Error>> {
        let cwd = std::env::current_dir()?;
        let capture = ShellCheck::new("sleep 0.1", &cwd, &Checker::default()).run()?;
        assert!(!capture.cancelled, "{capture:?}");
        assert_eq!(capture.exit_code, Some(0), "{capture:?}");
        Ok(())
    }
}
