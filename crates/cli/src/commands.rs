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

pub(crate) enum Words {
    Any,
    Only(&'static [&'static str]),
}
use Words::{Any, Only};

pub(crate) struct Verb(&'static str, Route, Words, &'static str);

pub(crate) struct Flag(&'static str, Option<&'static str>, &'static str);

#[rustfmt::skip]
const VERBS: &[Verb] = &[
    Verb("ask", Run(run_ask), Any, "run one turn and print the answer"),
    Verb("rpc", Run(run_rpc), Only(&[]), "speak the Pi JSON-RPC protocol on stdio"),
    Verb("acp", Run(run_acp), Only(&[]), "speak ACP v2 on stdio"),
    Verb("undo", Run(run_undo), Only(&["list"]), "restore the files the last turn changed"),
    Verb("lanes", Run(lanes::run_lanes), Any, "list the lane slots, or reap one"),
    Verb("trust", Run(run_trust), Any, "trust this repository, or list or revoke"),
    Verb("gate", Run(run_gate), Any, "say whether a command would run, and why"),
    Verb("fetch", Run(fetch::run), Any, "resolve one URL through the wall"),
    Verb("catalog", Run(catalog::run), Any, "show or refresh the model catalog"),
    Verb("doctor", Run(doctor::run), Only(&[]), "check session invariants, --fix repairs"),
    Verb("gc", Run(run_gc), Only(&[]), "preview the retention sweep, --fix runs it"),
    Verb("debug", Run(debug::run), Any, "info, paths, config, profile, trace, select"),
    Verb("why", Run(run_why), Any, "trace a line or a todo back to its goal"),
    Verb("plan", Run(run_plan), Any, "plan ops as the owner"),
    Verb("todo", Run(todo::run), Any, "print the newest todo list"),
    Verb("memory", Run(memory::run), Any, "list, search, import or forget memories"),
    Verb("sessions", Run(run_sessions), Any, "list, show, or remove stored sessions"),
    Verb("stats", Run(run_stats), Any, "tools and tokens of a session, or a rollup"),
    Verb("serve", Run(run_serve_command), Only(&[]), "run the session daemon"),
    Verb("console", Run(run_console_command), Only(&[]), "open the workspace shell"),
    Verb("tui", Run(run_tui), Any, "open the solo terminal UI"),
    Verb("version", Run(run_version), Only(&[]), "print the version"),
    Verb("update", Early, Any, "replace this install from the latest release"),
    Verb("plugin", Early, Any, "list, add, remove or sync pinned programs"),
    Verb("login", Early, Any, "store a provider credential"),
    Verb("logout", Early, Any, "remove a provider credential"),
    Verb("setup", Early, Any, "choose a model and a permission mode"),
    Verb("mcp", Early, Any, "one-shot MCP client calls, under mcp.enabled"),
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
    if let Only(words) = verb.2
        && !(args.prompt.is_empty() || words.contains(&args.prompt.as_str()))
    {
        let takes = match words {
            [] => "no words".to_owned(),
            words => format!("only `{}`", words.join("`, `")),
        };
        eprintln!(
            "error: `yi {0}` takes {takes}; to ask the model: yi ask {0} {1}",
            verb.0, args.prompt
        );
        return 2;
    }
    if let Some((flag, remedy)) = unread(verb.0, args) {
        eprintln!("error: --{flag} is not applied by yi {}; {remedy}", verb.0);
        return 2;
    }
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

/// Incident: `serve_flags` hands a worker only the model, the session dir and the mode, and the
/// console only reaches the daemon, so these flags were dropped without a word.
const UNREAD: &[(&str, &str)] = &[
    ("thinking", "set \"thinking\" in ~/.yi/config.json"),
    ("system", "pass it to `yi ask` or `yi tui`"),
    ("schema", "pass it to `yi ask`"),
    ("deadline", "pass it to `yi ask` or a headless drive"),
];

fn unread(verb: &str, args: &Args) -> Option<(&'static str, &'static str)> {
    let reads_deadline = verb == "console" && args.headless;
    UNREAD.iter().copied().find(|(flag, _)| {
        matches!(verb, "console" | "serve")
            && args.flags.iter().any(|given| given == flag)
            && !(reads_deadline && *flag == "deadline")
    })
}

/// A lone word this close to a verb is a typo, not a prompt: one edit at four or five letters,
/// two from six, none below four, where real words sit one edit from `gc`, `why` or `tui`.
pub(crate) fn near(word: &str) -> Option<&'static str> {
    let bound = match word.chars().count() {
        0..=3 => return None,
        4 | 5 => 1,
        _ => 2,
    };
    VERBS
        .iter()
        .map(|verb| (distance(word, verb.0), verb.0))
        .filter(|(edits, _)| *edits <= bound)
        .min()
        .map(|(_, name)| name)
}

fn distance(from: &str, to: &str) -> usize {
    let to: Vec<char> = to.chars().collect();
    let mut row: Vec<usize> = (0..=to.len()).collect();
    for (index, left) in from.chars().enumerate() {
        let mut next = vec![index.saturating_add(1)];
        for ((swap, up), right) in row.iter().zip(row.iter().skip(1)).zip(&to) {
            let beside = next.last().copied().unwrap_or(usize::MAX);
            let swapped = swap.saturating_add(usize::from(left != *right));
            next.push(swapped.min(up.min(&beside).saturating_add(1)));
        }
        row = next;
    }
    row.last().copied().unwrap_or(usize::MAX)
}

pub(crate) fn help() -> String {
    let mut out = format!(
        "yi {}: `yi <words>` asks the model; bare `yi` on a terminal opens the console\n\nverbs:\n",
        env!("ARCHITECTURE_VERSION")
    );
    for Verb(name, _, _, summary) in VERBS {
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
