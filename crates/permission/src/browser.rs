use std::path::Path;

use crate::desktop::{Judged, names_any, session_of};

pub const PROGRAM: &str = "agent-browser";

pub const VERSION: &str = "0.41.0";

/// The session whose Chrome has the internet: the plugin's wrapper reads the same name.
pub const WEB: &str = "web";

const CLEAR: &[&str] = &["--clear"];

/// Invariant: Chrome runs in a Yi sandbox, so these reach only what that box holds; a verb or flag
/// missing here asks every time, as anything the outside daemon would do itself must: `read` fetches
/// from outside, and `pdf`, `download`, `record`, `trace` and `har` write where the model says.
const VERBS: [(&str, &[&str]); 29] = [
    ("back", &[]),
    ("check", &[]),
    ("click", &[]),
    ("close", &["--all"]),
    ("console", CLEAR),
    ("dblclick", &[]),
    ("dialog", &[]),
    ("drag", &[]),
    ("errors", CLEAR),
    ("eval", &[]),
    ("fill", &[]),
    ("find", &["--exact", "--name"]),
    ("focus", &[]),
    ("forward", &[]),
    ("get", &[]),
    ("hover", &[]),
    ("is", &[]),
    ("keyboard", &[]),
    ("open", &[]),
    ("press", &[]),
    ("reload", &[]),
    ("screenshot", &["--annotate", "--full", "-f"]),
    ("scroll", &[]),
    ("scrollintoview", &[]),
    ("select", &[]),
    (
        "snapshot",
        &[
            "--compact",
            "--depth",
            "--interactive",
            "--selector",
            "--urls",
            "-c",
            "-d",
            "-i",
            "-s",
            "-u",
        ],
    ),
    ("type", &[]),
    ("uncheck", &[]),
    ("wait", &["--fn", "--load", "--text", "--timeout", "--url"]),
];

/// Invariant: each attaches the person's real logins or puts a window in front of them, neither of
/// which the box can hold, so none runs in any mode.
const REFUSED: [&str; 9] = [
    "--auto-connect",
    "--cdp",
    "--headed",
    "--profile",
    "--restore",
    "--session-name",
    "--state",
    "connect",
    "inspect",
];

/// Whether the pinned program stands beside the config and script that hold its Chrome. Invariant:
/// only `yi plugin add` writes under this directory, and it writes these last.
pub fn boxed(home: &Path) -> bool {
    let dir = home.join(".yi/plugins").join(PROGRAM).join(VERSION);
    [PROGRAM, "config.json", "chrome-box"]
        .iter()
        .all(|file| dir.join(file).is_file())
}

/// Invariant: only the plain name is judged (None otherwise), so `AGENT_BROWSER_EXECUTABLE_PATH=x
/// agent-browser` stays in the sandbox, where a daemon cannot bind its socket.
pub(crate) fn judge(argv: &[String]) -> Option<Judged> {
    if argv.first().map(String::as_str) != Some(PROGRAM) {
        return None;
    }
    let boxed = std::env::var_os("HOME").is_some_and(|home| boxed(Path::new(&home)));
    Some(judge_words(argv.get(1..).unwrap_or_default(), boxed))
}

fn judge_words(all: &[String], boxed: bool) -> Judged {
    if names_any(all, &REFUSED) {
        return Judged::Refused(format!(
            "this attaches the user's real logins or takes their screen; use the headless browser: {}",
            all.join(" ")
        ));
    }
    // Incident: a Chrome outside the box returned a planted `~/.ssh/id_rsa`; a copy on PATH, or an
    // install whose Chrome download failed, starts exactly that one.
    if !boxed {
        return Judged::Refused(format!(
            "agent-browser runs only from `yi plugin add agent-browser`, which holds its Chrome in a sandbox: {}",
            all.join(" ")
        ));
    }
    let (session, words) = session_of(all);
    let web = match session.as_deref() {
        None => false,
        Some(WEB) => true,
        Some(_) => return Judged::EachTime,
    };
    let words: Vec<&str> = (words.iter().map(String::as_str))
        .filter(|word| *word != "--json")
        .collect();
    let Some((verb, rest)) = words.split_first() else {
        return Judged::EachTime;
    };
    let Some((_, flags)) = VERBS.iter().find(|(name, _)| name == verb) else {
        return Judged::EachTime;
    };
    let unlisted = |word: &&str| {
        word.starts_with('-') && !flags.contains(&word.split('=').next().unwrap_or(word))
    };
    // Invariant: a screenshot's other word can be a path, `-f=x` included, which the outside daemon
    // would write.
    let writes = *verb == "screenshot"
        && (rest.iter()).any(|word| !word.starts_with('@') && !flags.contains(word));
    if rest.iter().any(unlisted) || writes {
        return Judged::EachTime;
    }
    if web {
        Judged::App(WEB.to_owned())
    } else {
        Judged::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judged(words: &[&str]) -> Option<Judged> {
        let words: Vec<String> = words.iter().copied().map(str::to_owned).collect();
        Some(judge_words(&words, true))
    }

    #[test]
    fn the_boxed_local_browser_needs_no_question() {
        for words in [
            &["open", "http://localhost:3000"][..],
            &["snapshot", "-i", "--compact"][..],
            &["click", "@e3"][..],
            &["fill", "@e2", "hello"][..],
            &["screenshot", "--full"][..],
            &["screenshot", "@e3"][..],
            &["--json", "get", "title"][..],
            &["close", "--all"][..],
        ] {
            assert_eq!(judged(words), Some(Judged::Free), "{words:?}");
        }
    }

    #[test]
    fn the_web_session_is_one_scope_whatever_the_verb_or_spelling() {
        let web = Some(Judged::App(WEB.to_owned()));
        for words in [
            &["--session", "web", "open", "https://example.com"][..],
            &["--session=web", "get", "title"][..],
            &["click", "@e3", "--session", "web"][..],
        ] {
            assert_eq!(judged(words), web, "{words:?}");
        }
    }

    #[test]
    fn whatever_the_table_cannot_vouch_for_asks_every_time() {
        for words in [
            &[][..],
            &["--executable-path", "/tmp/x", "open", "http://localhost"][..],
            &["--args", "--disable-web-security", "open", "x"][..],
            &["open", "http://localhost", "--proxy", "http://evil"][..],
            &["--config", "/tmp/x.json", "open", "x"][..],
            &["--session", "other", "open", "x"][..],
            &["--session", "../web", "get", "title"][..],
            &["read", "https://example.com"][..],
            &["screenshot", "/Users/u/.zshrc"][..],
            &["screenshot", "-f=/Users/u/.zshrc"][..],
            &["screenshot", "--full=/Users/u/.zshrc"][..],
            &["pdf", "x.pdf"][..],
            &["wait", "--download", "x.zip"][..],
            &["download", "@e1", "x"][..],
            &["upload", "@e1", "/etc/hosts"][..],
            &["cookies", "get"][..],
            &["auth", "list"][..],
            &["plugin", "list"][..],
            &["upgrade"][..],
            &["a-verb-from-a-later-version"][..],
            &["fill", "@e1", "--", "--text"][..],
        ] {
            assert_eq!(judged(words), Some(Judged::EachTime), "{words:?}");
        }
    }

    #[test]
    fn real_logins_and_a_visible_window_are_refused() {
        for words in [
            &["connect", "9222"][..],
            &["--json", "connect", "9222"][..],
            &["--auto-connect", "open", "x"][..],
            &["open", "x", "--cdp", "9222"][..],
            &["open", "x", "--cdp=9222"][..],
            &["--profile", "Default", "open", "x"][..],
            &["--state", "s.json", "open", "x"][..],
            &["--session-name", "mine", "open", "x"][..],
            &["--headed", "open", "x"][..],
            &["inspect"][..],
            &["--session", "web", "--headed", "open", "x"][..],
        ] {
            assert!(
                matches!(judged(words), Some(Judged::Refused(_))),
                "{words:?}"
            );
        }
        assert_eq!(judged(&["fill", "@e1", "connect"]), Some(Judged::Free));
    }

    #[test]
    fn with_no_box_nothing_runs_outside_whichever_session() {
        for words in [
            &["open", "http://localhost:3000"][..],
            &["--session", "web", "open", "https://example.com"][..],
            &["read", "https://example.com"][..],
        ] {
            let words: Vec<String> = words.iter().copied().map(str::to_owned).collect();
            assert!(
                matches!(judge_words(&words, false), Judged::Refused(why) if why.contains("yi plugin add")),
                "{words:?}"
            );
        }
    }

    #[test]
    fn the_box_is_the_program_its_config_and_its_script_together()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = std::env::temp_dir().join(format!("yi-browser-box-{}", std::process::id()));
        let dir = home.join(".yi/plugins").join(PROGRAM).join(VERSION);
        std::fs::create_dir_all(&dir)?;
        for file in [PROGRAM, "config.json", "chrome-box"] {
            assert!(!boxed(&home), "boxed before {file}");
            std::fs::write(dir.join(file), "")?;
        }
        assert!(boxed(&home));
        std::fs::remove_dir_all(&home)?;
        Ok(())
    }

    #[test]
    fn only_the_plain_program_name_is_judged() {
        for argv in [
            vec!["./agent-browser", "open", "x"],
            vec!["/tmp/agent-browser", "open", "x"],
            vec!["AGENT_BROWSER_EXECUTABLE_PATH=/x", "agent-browser", "open"],
            vec!["timeout", "5", "agent-browser", "open", "x"],
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
            assert_eq!(judge(&argv), None, "{argv:?}");
        }
    }
}
