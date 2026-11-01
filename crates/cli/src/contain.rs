//! `yi __contain`: the process a contained command starts as on Linux. It restricts itself and
//! execs, so no `pre_exec` and no `unsafe`: a ruleset outlives exec, and only tightens.

/// Before anything else in `main`: the hidden verbs, and the wrapper every later spawn names.
pub fn early() {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "__fds") {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(2).collect();
        eprintln!("error: __fds: {}", yi_browser::exec_on_pipe_fds(&args));
        std::process::exit(1);
    }
    #[cfg(feature = "tui")]
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "__pty") {
        std::process::exit(yi_console::pty::shim(std::env::args_os().skip(2)));
    }
    #[cfg(target_os = "linux")]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().is_some_and(|arg| arg == "__contain") {
            std::process::exit(linux::run(args));
        }
        if let Ok(exe) = std::env::current_exe() {
            yi_runtime::set_sandbox_wrapper(exe);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {

    use std::ffi::OsString;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;

    use landlock::{
        ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
        path_beneath_rules,
    };

    /// Exit code for "the sandbox did not apply", as a shell's for a command it could not run.
    const NOT_CONTAINED: i32 = 126;

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Plan {
        probe: bool,
        read: Vec<PathBuf>,
        write: Vec<PathBuf>,
        argv: Vec<OsString>,
    }

    fn parse(args: impl Iterator<Item = OsString>) -> Result<Plan, String> {
        let mut plan = Plan::default();
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--probe") => plan.probe = true,
                Some("--read") => plan
                    .read
                    .push(args.next().ok_or("--read needs a path")?.into()),
                Some("--write") => plan
                    .write
                    .push(args.next().ok_or("--write needs a path")?.into()),
                Some("--") => {
                    plan.argv = args.collect();
                    return Ok(plan);
                }
                _ => return Err(format!("unknown argument {}", arg.to_string_lossy())),
            }
        }
        if plan.probe {
            return Ok(plan);
        }
        Err("no command after --".to_owned())
    }

    /// Network stays open: Landlock's port rules would break the loopback test servers a build starts.
    fn restrict(read: &[PathBuf], write: &[PathBuf]) -> Result<(), String> {
        let abi = ABI::V3;
        let reason = |error: landlock::RulesetError| error.to_string();
        let status = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))
            .map_err(reason)?
            .create()
            .map_err(reason)?
            .add_rules(path_beneath_rules(read, AccessFs::from_read(abi)))
            .map_err(reason)?
            .add_rules(path_beneath_rules(write, AccessFs::from_all(abi)))
            .map_err(reason)?
            .restrict_self()
            .map_err(reason)?;
        match status.ruleset {
            RulesetStatus::NotEnforced => {
                Err("this kernel enforces no Landlock ruleset".to_owned())
            }
            RulesetStatus::FullyEnforced | RulesetStatus::PartiallyEnforced => Ok(()),
        }
    }

    /// Never returns on success. Every failure names itself and exits, so nothing runs uncontained.
    pub fn run(args: impl Iterator<Item = OsString>) -> i32 {
        let fail = |why: &str| {
            eprintln!("yi __contain: {why}");
            NOT_CONTAINED
        };
        let plan = match parse(args) {
            Ok(plan) => plan,
            Err(why) => return fail(&why),
        };
        if let Err(why) = restrict(&plan.read, &plan.write) {
            return fail(&why);
        }
        if plan.probe {
            return 0;
        }
        let Some((program, rest)) = plan.argv.split_first() else {
            return fail("no command after --");
        };
        let error = std::process::Command::new(program).args(rest).exec();
        fail(&format!("{}: {error}", program.to_string_lossy()))
    }
}
