use crate::desktop::Judged;

pub const PROGRAM: &str = "agent-browser";

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

/// A flag before the verb hides it, so then any word naming one counts; after a verb only a flag
/// does, since other words are typed text.
fn refused(all: &[String]) -> bool {
    let named = |word: &String| REFUSED.contains(&word.split('=').next().unwrap_or(word));
    match all.first() {
        Some(verb) if !verb.starts_with('-') => {
            named(verb) || all.iter().any(|word| word.starts_with("--") && named(word))
        }
        _ => all.iter().any(named),
    }
}

/// `--session`'s value when it is given, and the words without it or `--json`.
fn globals(all: &[String]) -> (Option<&str>, Vec<&str>) {
    let (mut session, mut rest) = (None, Vec::new());
    let mut words = all.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--session" {
            session = Some(words.next().unwrap_or_default());
        } else if let Some(name) = word.strip_prefix("--session=") {
            session = Some(name);
        } else if word != "--json" {
            rest.push(word);
        }
    }
    (session, rest)
}

/// Invariant: only the plain name is judged (None otherwise), so `AGENT_BROWSER_EXECUTABLE_PATH=x
/// agent-browser` stays in the sandbox, where a daemon cannot bind its socket.
pub(crate) fn judge(argv: &[String]) -> Option<Judged> {
    if argv.first().map(String::as_str) != Some(PROGRAM) {
        return None;
    }
    let all = argv.get(1..).unwrap_or_default();
    if refused(all) {
        return Some(Judged::Refused(format!(
            "this attaches the user's real logins or takes their screen; use the headless browser: {}",
            all.join(" ")
        )));
    }
    let (session, words) = globals(all);
    let web = match session {
        None => false,
        Some(WEB) => true,
        Some(_) => return Some(Judged::EachTime),
    };
    let Some((verb, rest)) = words.split_first() else {
        return Some(Judged::EachTime);
    };
    let Some((_, flags)) = VERBS.iter().find(|(name, _)| name == verb) else {
        return Some(Judged::EachTime);
    };
    let unlisted = |word: &&str| {
        word.starts_with('-') && !flags.contains(&word.split('=').next().unwrap_or(word))
    };
    // Invariant: a screenshot's other word can be a path, which the outside daemon would write.
    let writes = *verb == "screenshot" && rest.iter().any(|word| !word.starts_with(['-', '@']));
    if rest.iter().any(unlisted) || writes {
        return Some(Judged::EachTime);
    }
    Some(if web {
        Judged::App(WEB.to_owned())
    } else {
        Judged::Free
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judged(words: &[&str]) -> Option<Judged> {
        let argv: Vec<String> = std::iter::once(PROGRAM)
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect();
        judge(&argv)
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
