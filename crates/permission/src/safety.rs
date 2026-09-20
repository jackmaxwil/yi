#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Safe,
    Destructive,
    Egress,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Run it inside the platform sandbox where one exists, ask where it does
    /// not. Phase 1 has no sandbox, so every caller asks.
    Contain {
        reason: String,
    },
    Ask {
        reason: String,
    },
}

const SAFE: [&str; 43] = [
    "base64",
    "basename",
    "cat",
    "cd",
    "cksum",
    "comm",
    "cut",
    "date",
    "df",
    "diff",
    "dirname",
    "du",
    "echo",
    "false",
    "fd",
    "file",
    "head",
    "hostname",
    "id",
    "jq",
    "ls",
    "md5sum",
    "nl",
    "od",
    "printenv",
    "printf",
    "pwd",
    "readlink",
    "realpath",
    "rg",
    "sha1sum",
    "sha256sum",
    "sort",
    "stat",
    "tail",
    "tr",
    "tree",
    "true",
    "type",
    "uname",
    "uniq",
    "wc",
    "which",
];

/// Recognized irreversible verbs. A hit asks, in every mode below yolo, even
/// inside the worktree: `rm -rf src/` is gone for anything untracked.
const DESTRUCTIVE: [&str; 26] = [
    "chgrp",
    "chmod",
    "chown",
    "crontab",
    "dd",
    "diskutil",
    "halt",
    "killall",
    "launchctl",
    "mkfs",
    "nc",
    "pkill",
    "poweroff",
    "reboot",
    "rm",
    "rmdir",
    "rsync",
    "scp",
    "shred",
    "shutdown",
    "ssh",
    "sudo",
    "systemctl",
    "tmutil",
    "truncate",
    "unlink",
];

const LAUNDERING: [&str; 12] = [
    "bash", "dash", "doas", "env", "eval", "exec", "sh", "source", "su", "watch", "xargs", "zsh",
];

const PASSTHROUGH: [&str; 4] = ["ionice", "nice", "stdbuf", "time"];

const GIT_READ: [&str; 10] = [
    "blame",
    "describe",
    "diff",
    "log",
    "ls-files",
    "remote",
    "rev-parse",
    "shortlog",
    "show",
    "status",
];

const CARGO_READ: [&str; 9] = [
    "build",
    "check",
    "clippy",
    "doc",
    "fmt",
    "metadata",
    "test",
    "tree",
    "verify-project",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Segments(Vec<Vec<String>>),
    Unparsed,
}

const BAIL: [char; 9] = ['$', '`', '<', '>', '(', ')', '{', '}', '\n'];

pub fn parse(command: &str) -> Parsed {
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut argv: Vec<String> = Vec::new();
    let mut token = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let mut quoted_token = false;
    while let Some(character) = chars.next() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            } else if character == '\\' && open == '"' {
                match chars.next() {
                    Some(escaped) => token.push(escaped),
                    None => return Parsed::Unparsed,
                }
            } else {
                token.push(character);
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                quoted_token = true;
            }
            '\\' => match chars.next() {
                Some(escaped) => token.push(escaped),
                None => return Parsed::Unparsed,
            },
            character if BAIL.contains(&character) => return Parsed::Unparsed,
            '&' | '|' | ';' => {
                if chars.peek() == Some(&character) {
                    chars.next();
                } else if character == '&' {
                    return Parsed::Unparsed;
                }
                push_token(&mut argv, &mut token, &mut quoted_token);
                if argv.is_empty() {
                    return Parsed::Unparsed;
                }
                segments.push(std::mem::take(&mut argv));
            }
            character if character.is_whitespace() => {
                push_token(&mut argv, &mut token, &mut quoted_token);
            }
            character => token.push(character),
        }
    }
    if quote.is_some() {
        return Parsed::Unparsed;
    }
    push_token(&mut argv, &mut token, &mut quoted_token);
    if !argv.is_empty() {
        segments.push(argv);
    }
    if segments.is_empty() {
        return Parsed::Unparsed;
    }
    Parsed::Segments(segments)
}

/// A quoted argv0 (`r''m`) is the oldest way past a string match, so a program
/// name that was assembled from quotes is never a name Yi recognizes.
fn push_token(argv: &mut Vec<String>, token: &mut String, quoted: &mut bool) {
    if token.is_empty() && !*quoted {
        return;
    }
    if *quoted && argv.is_empty() {
        argv.push(format!("\u{0}{token}"));
    } else {
        argv.push(std::mem::take(token));
    }
    token.clear();
    *quoted = false;
}

fn program(argv0: &str) -> &str {
    argv0.rsplit('/').next().unwrap_or(argv0)
}

fn flags(argv: &[String]) -> Vec<&str> {
    argv.iter()
        .skip(1)
        .map(String::as_str)
        .filter(|token| token.starts_with('-'))
        .collect()
}

fn subcommand(argv: &[String]) -> Option<&str> {
    argv.iter()
        .skip(1)
        .map(String::as_str)
        .find(|token| !token.starts_with('-'))
}

const GIT_VALUED_OPTIONS: [&str; 6] = [
    "--config-env",
    "--git-dir",
    "--namespace",
    "--work-tree",
    "-C",
    "-c",
];

pub(crate) fn git_subcommand(argv: &[String]) -> Option<&str> {
    let mut tokens = argv.iter().skip(1).map(String::as_str);
    while let Some(token) = tokens.next() {
        if GIT_VALUED_OPTIONS.contains(&token) {
            tokens.next();
        } else if !token.starts_with('-') {
            return Some(token);
        }
    }
    None
}

fn git_class(argv: &[String]) -> Class {
    let Some(subcommand) = git_subcommand(argv) else {
        return Class::Safe;
    };
    let flags = flags(argv);
    let has = |flag: &str| flags.contains(&flag);
    let destructive = match subcommand {
        "reset" => has("--hard") || has("--merge") || has("--keep"),
        "clean" => has("-f") || has("-d") || has("-x") || has("-fd") || has("-fdx"),
        "checkout" | "switch" => argv.iter().any(|token| token == "--") || has("--force"),
        "restore" | "rebase" | "filter-branch" | "reflog" | "gc" | "prune" => true,
        "push" => {
            flags.iter().any(|flag| flag.starts_with("--force"))
                || has("-f")
                || has("--delete")
                || has("--prune")
        }
        "branch" | "tag" => has("-D") || has("-d") || has("--delete"),
        "stash" => matches!(
            subcommand_after(argv, "stash"),
            Some("drop" | "clear" | "pop")
        ),
        _ => false,
    };
    if destructive {
        return Class::Destructive;
    }
    if matches!(
        subcommand,
        "clone" | "fetch" | "ls-remote" | "pull" | "push" | "submodule"
    ) {
        return Class::Egress;
    }
    if GIT_READ.binary_search(&subcommand).is_ok() {
        return Class::Safe;
    }
    if subcommand == "branch" || subcommand == "tag" {
        return Class::Safe;
    }
    if subcommand == "stash" && matches!(subcommand_after(argv, "stash"), Some("list" | "show")) {
        return Class::Safe;
    }
    Class::Unknown
}

fn subcommand_after<'a>(argv: &'a [String], after: &str) -> Option<&'a str> {
    let index = argv.iter().position(|token| token == after)?;
    argv.get(index.saturating_add(1))
        .map(String::as_str)
        .filter(|token| !token.starts_with('-'))
}

fn cargo_class(argv: &[String]) -> Class {
    match subcommand(argv) {
        Some("install" | "uninstall" | "publish" | "yank" | "login") => Class::Destructive,
        Some(subcommand) if CARGO_READ.binary_search(&subcommand).is_ok() => Class::Safe,
        _ => Class::Unknown,
    }
}

fn find_class(argv: &[String]) -> Class {
    let destructive = argv.iter().any(|token| {
        matches!(
            token.as_str(),
            "-delete" | "-exec" | "-execdir" | "-ok" | "-fprintf" | "-fls"
        )
    });
    if destructive {
        Class::Destructive
    } else {
        Class::Safe
    }
}

fn network_class(argv: &[String]) -> Class {
    let writes = argv.iter().any(|token| {
        matches!(
            token.as_str(),
            "-d" | "--data"
                | "--data-binary"
                | "-F"
                | "--form"
                | "-T"
                | "--upload-file"
                | "-o"
                | "--output"
                | "-O"
        )
    });
    let method = argv
        .iter()
        .position(|token| token == "-X" || token == "--request")
        .and_then(|index| argv.get(index.saturating_add(1)))
        .is_some_and(|method| !method.eq_ignore_ascii_case("get"));
    if writes || method {
        Class::Destructive
    } else {
        Class::Unknown
    }
}

pub fn classify(argv: &[String]) -> Class {
    let Some(argv0) = argv.first() else {
        return Class::Unknown;
    };
    if argv0.starts_with('\u{0}') {
        return Class::Unknown;
    }
    let name = program(argv0);
    if PASSTHROUGH.binary_search(&name).is_ok() {
        return classify(&argv[1..]);
    }
    if name == "timeout" {
        return classify(argv.get(2..).unwrap_or_default());
    }
    if LAUNDERING.binary_search(&name).is_ok() {
        return Class::Destructive;
    }
    if DESTRUCTIVE.binary_search(&name).is_ok() {
        return Class::Destructive;
    }
    match name {
        "git" => git_class(argv),
        "cargo" => cargo_class(argv),
        "find" => find_class(argv),
        "curl" | "wget" | "http" => network_class(argv),
        "sed" => {
            if flags(argv).iter().any(|flag| flag.starts_with("-i")) {
                Class::Unknown
            } else {
                Class::Safe
            }
        }
        "grep" | "awk" => Class::Safe,
        "npm" | "pnpm" | "yarn" | "pip" | "pip3" | "brew" | "gem" => match subcommand(argv) {
            Some("install" | "uninstall" | "remove" | "add" | "publish" | "link") => {
                Class::Destructive
            }
            _ => Class::Unknown,
        },
        name if SAFE.binary_search(&name).is_ok() => Class::Safe,
        _ => Class::Unknown,
    }
}

/// The ladder: any destructive segment asks, any unknown segment is contained,
/// and allow needs positive proof from every segment.
pub fn verdict(command: &str) -> Verdict {
    let Parsed::Segments(segments) = parse(command) else {
        return Verdict::Contain {
            reason: "the command uses shell expansion, redirection, or control flow that Yi does not read statically".to_owned(),
        };
    };
    let mut unknown: Option<String> = None;
    for argv in &segments {
        match classify(argv) {
            Class::Destructive => {
                return Verdict::Ask {
                    reason: format!(
                        "`{}` is destructive; it cannot be undone by a checkpoint",
                        argv.first().map_or("", String::as_str)
                    ),
                };
            }
            Class::Egress => {
                return Verdict::Ask {
                    reason: format!(
                        "`git {}` needs the network; a contained run has none",
                        git_subcommand(argv).unwrap_or_default()
                    ),
                };
            }
            Class::Unknown => {
                if unknown.is_none() {
                    unknown = Some(format!(
                        "`{}` is not in the known-safe set",
                        argv.first().map_or("", String::as_str)
                    ));
                }
            }
            Class::Safe => {}
        }
    }
    match unknown {
        Some(reason) => Verdict::Contain { reason },
        None => Verdict::Allow,
    }
}
