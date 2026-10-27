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

pub(crate) struct Verb {
    name: &'static str,
    route: Route,
    words: Words,
    summary: &'static str,
}

pub(crate) struct Flag {
    name: &'static str,
    value: Option<&'static str>,
    summary: &'static str,
}

#[rustfmt::skip]
const VERBS: &[Verb] = &[
    Verb { name: "ask", route: Run(run_ask), words: Any, summary: "run one turn and print the answer" },
    Verb { name: "rpc", route: Run(run_rpc), words: Only(&[]), summary: "speak the Pi JSON-RPC protocol on stdio" },
    Verb { name: "acp", route: Run(run_acp), words: Only(&[]), summary: "speak ACP v2 on stdio" },
    Verb { name: "undo", route: Run(run_undo), words: Only(&["list"]), summary: "restore the files the last turn changed" },
    Verb { name: "lanes", route: Run(lanes::run_lanes), words: Any, summary: "list the lane slots, or reap one" },
    Verb { name: "trust", route: Run(run_trust), words: Any, summary: "trust this repository, or list or revoke" },
    Verb { name: "gate", route: Run(run_gate), words: Any, summary: "say whether a command would run, and why" },
    Verb { name: "fetch", route: Run(fetch::run), words: Any, summary: "resolve one URL through the wall" },
    Verb { name: "catalog", route: Run(catalog::run), words: Any, summary: "show or refresh the model catalog" },
    Verb { name: "doctor", route: Run(doctor::run), words: Only(&[]), summary: "check session invariants, --fix repairs" },
    Verb { name: "gc", route: Run(run_gc), words: Only(&[]), summary: "preview the retention sweep, --fix runs it" },
    Verb { name: "debug", route: Run(debug::run), words: Any, summary: "info, paths, config, profile, trace, select" },
    Verb { name: "why", route: Run(run_why), words: Any, summary: "trace a line or a todo back to its goal" },
    Verb { name: "plan", route: Run(run_plan), words: Any, summary: "plan ops as the owner" },
    Verb { name: "todo", route: Run(todo::run), words: Any, summary: "print the newest todo list" },
    Verb { name: "memory", route: Run(memory::run), words: Any, summary: "list, search, import or forget memories" },
    Verb { name: "sessions", route: Run(run_sessions), words: Any, summary: "list, show, or remove stored sessions" },
    Verb { name: "stats", route: Run(run_stats), words: Any, summary: "tools and tokens of a session, or a rollup" },
    Verb { name: "serve", route: Run(run_serve_command), words: Only(&[]), summary: "run the session daemon" },
    Verb { name: "console", route: Run(run_console_command), words: Only(&[]), summary: "open the workspace shell" },
    Verb { name: "tui", route: Run(run_tui), words: Any, summary: "open the solo terminal UI" },
    Verb { name: "version", route: Run(run_version), words: Only(&[]), summary: "print the version" },
    Verb { name: "update", route: Early, words: Any, summary: "replace this install from the latest release" },
    Verb { name: "plugin", route: Early, words: Any, summary: "list, add, remove or sync pinned programs" },
    Verb { name: "login", route: Early, words: Any, summary: "store a provider credential" },
    Verb { name: "logout", route: Early, words: Any, summary: "remove a provider credential" },
    Verb { name: "setup", route: Early, words: Any, summary: "choose a model and a permission mode" },
    Verb { name: "mcp", route: Early, words: Any, summary: "one-shot MCP client calls, under mcp.enabled" },
];

#[rustfmt::skip]
const FLAGS: &[Flag] = &[
    Flag { name: "help", value: None, summary: "print this table (also -h)" },
    Flag { name: "version", value: None, summary: "print the version" },
    Flag { name: "model", value: Some("provider/id"), summary: "the model, over the configured one" },
    Flag { name: "system", value: Some("text"), summary: "extra system prompt" },
    Flag { name: "thinking", value: Some("level"), summary: "off, minimal, low, medium, high, xhigh, max" },
    Flag { name: "json", value: None, summary: "print JSON lines (the event stream, for ask)" },
    Flag { name: "no-deltas", value: None, summary: "with ask --json: drop the per-token message_update lines" },
    Flag { name: "yolo", value: None, summary: "run without permission prompts" },
    Flag { name: "auto", value: None, summary: "let the policy judge each call (the default)" },
    Flag { name: "confirm", value: None, summary: "ask before every write or command" },
    Flag { name: "allow-write", value: Some("dir"), summary: "let the sandbox write dir too; repeatable" },
    Flag { name: "session-dir", value: Some("dir"), summary: "session storage root" },
    Flag { name: "cwd", value: Some("dir"), summary: "working directory" },
    Flag { name: "here", value: None, summary: "work in this directory and claim no lane" },
    Flag { name: "fix", value: None, summary: "let doctor or gc change what it reports" },
    Flag { name: "socket", value: Some("path"), summary: "daemon socket path" },
    Flag { name: "headless", value: None, summary: "drive the tui or console without a terminal" },
    Flag { name: "lanes", value: None, summary: "claim a lane in a headless or cassette run" },
    Flag { name: "solo", value: None, summary: "bare yi opens the tui, not the console" },
    Flag { name: "keys", value: Some("file"), summary: "headless key script" },
    Flag { name: "control", value: Some("socket"), summary: "drive an interactive run from a socket" },
    Flag { name: "frames", value: Some("dir"), summary: "headless frame dump directory" },
    Flag { name: "faux", value: Some("file"), summary: "replay a reply cassette, no model call" },
    Flag { name: "record", value: Some("file"), summary: "record the headless drive as an asciicast" },
    Flag { name: "snap", value: Some("file"), summary: "one still frame of the headless drive" },
    Flag { name: "size", value: Some("COLSxROWS"), summary: "headless screen size, 80x24 by default" },
    Flag { name: "eval", value: None, summary: "an eval harness run, which reads YI_LEVERS" },
    Flag { name: "deadline", value: Some("secs"), summary: "wall-clock limit for the run" },
    Flag { name: "continue", value: None, summary: "resume the newest session for this directory" },
    Flag { name: "session", value: Some("id"), summary: "resume this session" },
    Flag { name: "schema", value: Some("schema"), summary: "answer JSON valid against this schema" },
    Flag { name: "since", value: Some("span"), summary: "after stats: fold the sessions of this window" },
    Flag { name: "by", value: Some("axis"), summary: "after stats: group by model, upstream, day, prices or plan" },
    Flag { name: "top", value: Some("n"), summary: "after stats: list the n costliest sessions" },
];

pub(crate) fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|verb| verb.name == name)
}

pub(crate) fn find_flag(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|flag| flag.name == name)
}

pub(crate) fn run(verb: &Verb, args: &Args) -> i32 {
    if let Only(words) = verb.words
        && !(args.prompt.is_empty() || words.contains(&args.prompt.as_str()))
    {
        let takes = match words {
            [] => "no words".to_owned(),
            words => format!("only `{}`", words.join("`, `")),
        };
        eprintln!(
            "error: `yi {0}` takes {takes}; to ask the model: yi ask {0} {1}",
            verb.name, args.prompt
        );
        return 2;
    }
    if let Some((flag, remedy)) = unread(verb.name, args) {
        eprintln!(
            "error: --{flag} is not applied by yi {}; {remedy}",
            verb.name
        );
        return 2;
    }
    match verb.route {
        Run(run) => run(args),
        Early => {
            eprintln!(
                "error: `yi {0}` reads its own flags: put it first, `yi {0} ...`",
                verb.name
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
        .map(|verb| (distance(word, verb.name), verb.name))
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
    for Verb { name, summary, .. } in VERBS {
        out.push_str(&format!("  {name:<10}{summary}\n"));
    }
    out.push_str("\nflags:\n");
    for Flag {
        name,
        value,
        summary,
    } in FLAGS
    {
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
        for super::Flag { name, value, .. } in FLAGS {
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
