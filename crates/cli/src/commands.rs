use crate::{
    Args, catalog, debug, doctor, fetch, lanes, memory, run_acp, run_ask, run_console_command,
    run_gate, run_gc, run_plan, run_rpc, run_serve_command, run_sessions, run_stats, run_trust,
    run_tui, run_undo, run_version, run_why, todo,
};

pub(crate) enum Route {
    Run(fn(&Args) -> i32),
    /// Invariant: answered in `main` before the shared flags parse, so it lands here only when
    /// it was not the first word.
    Early,
}
use Route::{Early, Run};

pub(crate) struct Verb(&'static str, Route, &'static str);

pub(crate) struct Flag(&'static str, Option<&'static str>, &'static str);

#[rustfmt::skip]
const VERBS: &[Verb] = &[
    Verb("ask", Run(run_ask), "run one turn and print the answer"),
    Verb("rpc", Run(run_rpc), "speak the Pi JSON-RPC protocol on stdio"),
    Verb("acp", Run(run_acp), "speak ACP v2 on stdio"),
    Verb("undo", Run(run_undo), "restore the files the last turn changed"),
    Verb("lanes", Run(lanes::run_lanes), "list the lane slots, or reap one"),
    Verb("trust", Run(run_trust), "trust this repository, or list or revoke"),
    Verb("gate", Run(run_gate), "say whether a command would run, and why"),
    Verb("fetch", Run(fetch::run), "resolve one URL through the wall"),
    Verb("catalog", Run(catalog::run), "show or refresh the model catalog"),
    Verb("doctor", Run(doctor::run), "check session invariants, --fix repairs"),
    Verb("gc", Run(run_gc), "preview the retention sweep, --fix runs it"),
    Verb("debug", Run(debug::run), "info, paths, config, profile, trace, select"),
    Verb("why", Run(run_why), "trace a line or a todo back to its goal"),
    Verb("plan", Run(run_plan), "plan ops as the owner"),
    Verb("todo", Run(todo::run), "print the newest todo list"),
    Verb("memory", Run(memory::run), "list, search, import or forget memories"),
    Verb("sessions", Run(run_sessions), "list, show, or remove stored sessions"),
    Verb("stats", Run(run_stats), "tools and tokens of a session, or a rollup"),
    Verb("serve", Run(run_serve_command), "run the session daemon"),
    Verb("console", Run(run_console_command), "open the workspace shell"),
    Verb("tui", Run(run_tui), "open the solo terminal UI"),
    Verb("version", Run(run_version), "print the version"),
    Verb("update", Early, "replace this install from the latest release"),
    Verb("plugin", Early, "list, add, remove or sync pinned programs"),
    Verb("login", Early, "store a provider credential"),
    Verb("logout", Early, "remove a provider credential"),
    Verb("setup", Early, "choose a model and a permission mode"),
    Verb("mcp", Early, "one-shot MCP client calls, under mcp.enabled"),
];

#[rustfmt::skip]
const FLAGS: &[Flag] = &[
    Flag("help", None, "print this table (also -h)"),
    Flag("version", None, "print the version"),
    Flag("model", Some("provider/id"), "the model, over the configured one"),
    Flag("system", Some("text"), "extra system prompt"),
    Flag("thinking", Some("level"), "off, minimal, low, medium, high, xhigh, max"),
    Flag("json", None, "print JSON lines (the event stream, for ask)"),
    Flag("yolo", None, "run without permission prompts"),
    Flag("auto", None, "let the policy judge each call (the default)"),
    Flag("confirm", None, "ask before every write or command"),
    Flag("session-dir", Some("dir"), "session storage root"),
    Flag("cwd", Some("dir"), "working directory"),
    Flag("here", None, "work in this directory and claim no lane"),
    Flag("fix", None, "let doctor or gc change what it reports"),
    Flag("socket", Some("path"), "daemon socket path"),
    Flag("headless", None, "drive the tui or console without a terminal"),
    Flag("lanes", None, "claim a lane in a headless or cassette run"),
    Flag("solo", None, "bare yi opens the tui, not the console"),
    Flag("keys", Some("file"), "headless key script"),
    Flag("control", Some("socket"), "drive an interactive run from a socket"),
    Flag("frames", Some("dir"), "headless frame dump directory"),
    Flag("faux", Some("file"), "replay a reply cassette, no model call"),
    Flag("record", Some("file"), "record the headless drive as an asciicast"),
    Flag("snap", Some("file"), "one still frame of the headless drive"),
    Flag("size", Some("COLSxROWS"), "headless screen size, 80x24 by default"),
    Flag("eval", None, "an eval harness run, which reads YI_LEVERS"),
    Flag("deadline", Some("secs"), "wall-clock limit for the run"),
    Flag("continue", None, "resume the newest session for this directory"),
    Flag("session", Some("id"), "resume this session"),
    Flag("schema", Some("schema"), "answer JSON valid against this schema"),
    Flag("since", Some("span"), "fold stats over the sessions of this window"),
    Flag("by", Some("axis"), "group stats by model, upstream, day or prices"),
    Flag("top", Some("n"), "stats lists the n costliest sessions"),
];

pub(crate) fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|verb| verb.0 == name)
}

pub(crate) fn find_flag(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|flag| flag.0 == name)
}

pub(crate) fn run(verb: &Verb, args: &Args) -> i32 {
    match verb.1 {
        Run(run) => run(args),
        Early => {
            eprintln!(
                "error: `yi {0}` reads its own flags: put it first, `yi {0} ...`",
                verb.0
            );
            2
        }
    }
}

pub(crate) fn help() -> String {
    let mut out = format!(
        "yi {}: `yi <words>` asks the model; bare `yi` on a terminal opens the console\n\nverbs:\n",
        env!("ARCHITECTURE_VERSION")
    );
    for Verb(name, _, summary) in VERBS {
        out.push_str(&format!("  {name:<10}{summary}\n"));
    }
    out.push_str("\nflags:\n");
    for Flag(name, value, summary) in FLAGS {
        let named = value.map_or_else(
            || format!("--{name}"),
            |value| format!("--{name} <{value}>"),
        );
        out.push_str(&format!("  {named:<26}{summary}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::FLAGS;

    #[test]
    fn every_flag_help_lists_is_one_the_parser_takes() {
        for super::Flag(name, value, _) in FLAGS {
            let sample = match *name {
                "thinking" => "high",
                "size" => "80x24",
                "deadline" | "top" => "5",
                "since" => "7d",
                "by" => "model",
                _ => "x",
            };
            let flag = format!("--{name}");
            let argv = ["yi", "stats", flag.as_str()];
            let parsed = crate::args::parse_from(argv.into_iter().chain(value.map(|_| sample)));
            if let Err(error) = parsed {
                assert!(
                    !error.to_string().contains("invalid option"),
                    "--{name}: {error}"
                );
            }
        }
    }
}
