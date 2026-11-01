use std::path::{Path, PathBuf};

pub const PROGRAM: &str = "agent-desktop";

/// Verbs that read no app's contents, so they leave the sandbox unasked when they carry no flag.
const FREE: [&str; 6] = [
    "list-apps",
    "list-displays",
    "permissions",
    "skills",
    "status",
    "version",
];

const TIMEOUT: &[&str] = &["--timeout-ms"];

/// Invariant: a verb or flag missing here asks every time, so the table only ever under-grants;
/// `--trace` and `launch --env` are out for what they write or inject.
const VERBS: [(&str, &[&str]); 23] = [
    ("check", TIMEOUT),
    ("clear", TIMEOUT),
    ("click", TIMEOUT),
    ("close-app", &[]),
    ("collapse", TIMEOUT),
    ("expand", TIMEOUT),
    (
        "find",
        &[
            "--app",
            "--count",
            "--description",
            "--exact",
            "--first",
            "--last",
            "--limit",
            "--name",
            "--native-id",
            "--nth",
            "--role",
            "--root",
            "--state",
            "--surface",
            "--text",
            "--timeout-ms",
            "--value",
        ],
    ),
    ("focus", TIMEOUT),
    ("get", &["--property"]),
    ("is", &["--property"]),
    ("launch", &[]),
    ("list-surfaces", &["--app"]),
    ("list-windows", &["--app", "--timeout-ms"]),
    ("right-click", TIMEOUT),
    ("scroll", &["--amount", "--direction", "--timeout-ms"]),
    ("scroll-to", TIMEOUT),
    ("select", TIMEOUT),
    ("set-value", TIMEOUT),
    (
        "snapshot",
        &[
            "--app",
            "--compact",
            "--include-bounds",
            "--interactive-only",
            "--max-depth",
            "--root",
            "--skeleton",
            "--surface",
            "--timeout-ms",
            "-i",
        ],
    ),
    ("toggle", TIMEOUT),
    ("type", TIMEOUT),
    ("uncheck", TIMEOUT),
    (
        "wait",
        &[
            "--action",
            "--app",
            "--count",
            "--element",
            "--menu",
            "--menu-closed",
            "--predicate",
            "--text",
            "--timeout",
            "--value",
            "--window",
        ],
    ),
];

/// Display names, lowercase: a browser is driven by a browser driver, never through this program.
const BROWSERS: [&str; 13] = [
    "arc",
    "brave browser",
    "chrome",
    "chromium",
    "dia",
    "firefox",
    "google chrome",
    "microsoft edge",
    "opera",
    "orion",
    "safari",
    "vivaldi",
    "zen",
];

/// Invariant: an act here can run a command or reach a secret, so one yes would switch the sandbox
/// off for the session. Terminals, editors with a terminal, settings, passwords, script runners.
const NEVER_STANDING: [&str; 30] = [
    "1password",
    "alacritty",
    "automator",
    "bitwarden",
    "claude",
    "code",
    "cursor",
    "dashlane",
    "ghostty",
    "hyper",
    "intellij idea",
    "iterm",
    "iterm2",
    "keychain access",
    "kitty",
    "lastpass",
    "nova",
    "passwords",
    "rio",
    "rustrover",
    "script editor",
    "shortcuts",
    "system preferences",
    "system settings",
    "tabby",
    "terminal",
    "visual studio code",
    "warp",
    "wezterm",
    "xcode",
];

/// Invariant: each takes the person's own mouse, keyboard or front app, which they are using while
/// the agent works, so none runs in any mode. `batch` is here since its entries can be `press`.
const PHYSICAL: [&str; 15] = [
    "--activate",
    "--headed",
    "batch",
    "double-click",
    "drag",
    "hover",
    "key-down",
    "key-up",
    "mouse-click",
    "mouse-down",
    "mouse-move",
    "mouse-up",
    "mouse-wheel",
    "press",
    "triple-click",
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Judged {
    Free,
    /// One ordinary app, lowercase: a single session grant covers reads and acts in it.
    App(String),
    EachTime,
    /// Why no mode runs this.
    Refused(String),
}

/// Invariant: upstream's `--app` also takes a bundle id, and `com.apple.Terminal` is Terminal past
/// a list of display names, so a dot or a slash cannot be judged by name.
fn opaque(app: &str) -> bool {
    app.contains(['.', '/'])
}

fn state_root() -> Option<PathBuf> {
    std::env::var_os("AGENT_DESKTOP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".agent-desktop")))
}

/// The app a qualified ref was snapshotted in. Invariant: a contained command cannot write under the
/// state root, and an env prefix never reaches here, so the model cannot forge this record.
fn ref_app(token: &str, root: &Path) -> Option<String> {
    let (snapshot, element) = token.strip_prefix('@')?.split_once(':')?;
    // An id with anything but alphanumerics would otherwise be a path the model chose.
    if snapshot.is_empty() || !snapshot.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let json = std::fs::read(root.join("snapshots").join(snapshot).join("refmap.json")).ok()?;
    yi_types::desktop::source_app(&json, &format!("@{element}"))
}

/// The apps a command's words name, or None where a word cannot be judged.
fn targets(verb: &str, flags: &[&str], words: &[String], root: &Path) -> Option<Vec<String>> {
    let positional = matches!(verb, "launch" | "close-app");
    let mut apps = Vec::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        // Invariant: `--surface focused` reads whatever is in front, whichever app `--app` named.
        if word.ends_with("focused") {
            return None;
        }
        if word.starts_with('@') {
            apps.push(ref_app(word, root)?);
        } else if word.starts_with('-') {
            let (flag, inline) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(flag, value)| (flag, Some(value)));
            if !flags.contains(&flag) {
                return None;
            }
            if let Some(reference) = inline.filter(|value| value.starts_with('@')) {
                apps.push(ref_app(reference, root)?);
            } else if flag == "--app" {
                let app = inline.or_else(|| words.next().map(String::as_str))?;
                apps.push(app.to_owned());
            }
        } else if positional {
            apps.push(word.clone());
        }
    }
    Some(apps)
}

/// Invariant: only the plain name is judged (None otherwise), so `X=1 agent-desktop` and
/// `./agent-desktop` get no grant and stay in the sandbox, where the OS refuses them.
pub(crate) fn judge(argv: &[String]) -> Option<Judged> {
    judge_at(argv, &state_root().unwrap_or_default())
}

/// Whether a command takes physical input. A global flag before the verb hides it, so then any
/// word naming one counts; after a verb only a flag does, since other words are typed text.
fn takes_input(all: &[String]) -> bool {
    let named = |word: &String| PHYSICAL.contains(&word.split('=').next().unwrap_or(word));
    match all.first() {
        Some(verb) if !verb.starts_with('-') => {
            named(verb) || all.iter().any(|word| word.starts_with("--") && named(word))
        }
        _ => all.iter().any(named),
    }
}

/// `--session`'s value, and the words without it: a session's records live under its own directory.
fn session_of(words: &[String]) -> (Option<String>, Vec<String>) {
    let (mut session, mut rest) = (None, Vec::new());
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if word == "--session" {
            session = Some(words.next().cloned().unwrap_or_default());
        } else if let Some(id) = word.strip_prefix("--session=") {
            session = Some(id.to_owned());
        } else {
            rest.push(word.clone());
        }
    }
    (session, rest)
}

fn judge_at(argv: &[String], root: &Path) -> Option<Judged> {
    if argv.first().map(String::as_str) != Some(PROGRAM) {
        return None;
    }
    let all = argv.get(1..).unwrap_or_default();
    let verb = all.first().map_or("", String::as_str);
    let words = all.get(1..).unwrap_or_default();
    if takes_input(all) {
        return Some(Judged::Refused(format!(
            "this takes the user's own mouse, keyboard or front app; act on refs with no --headed: {}",
            all.join(" ")
        )));
    }
    let cursor: Vec<&str> = all.iter().map(String::as_str).collect();
    if FREE.contains(&verb) && !words.iter().any(|word| word.starts_with('-'))
        || matches!(
            cursor.as_slice(),
            ["session", "start", "--cursor"] | ["session", "end", "--session", _]
        )
    {
        return Some(Judged::Free);
    }
    let (session, words) = session_of(words);
    let root = match session {
        Some(id) if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
            return Some(Judged::EachTime);
        }
        Some(id) => root.join("sessions").join(id),
        None => root.to_path_buf(),
    };
    let root = root.as_path();
    let words = words.as_slice();
    let Some((_, flags)) = VERBS.iter().find(|(name, _)| *name == verb) else {
        return Some(Judged::EachTime);
    };
    let Some(apps) = targets(verb, flags, words, root) else {
        return Some(Judged::EachTime);
    };
    let apps: Vec<String> = apps.iter().map(|app| app.to_lowercase()).collect();
    if let Some(browser) = apps.iter().find(|app| BROWSERS.contains(&app.as_str())) {
        return Some(Judged::Refused(format!(
            "agent-desktop does not drive browsers ({browser}); use agent-browser for web pages"
        )));
    }
    Some(match apps.as_slice() {
        [first, rest @ ..]
            if rest.iter().all(|app| app == first)
                && !opaque(first)
                && !NEVER_STANDING.contains(&first.as_str()) =>
        {
            Judged::App(first.clone())
        }
        _ => Judged::EachTime,
    })
}

/// Whether any segment drives the desktop by any spelling and is not a proven free verb: such a
/// command is the user's to answer, never a review model's, and its output is an app's words.
pub fn drives(command: &str) -> bool {
    crate::safety::command_segments(command).iter().any(|argv| {
        let named = argv
            .iter()
            .any(|word| crate::safety::program(word) == PROGRAM);
        named && judge(argv) != Some(Judged::Free)
    })
}

/// Why no mode runs a segment: a browser, which the user routes elsewhere, or their own input.
pub(crate) fn refused(command: &str) -> Option<String> {
    crate::safety::command_segments(command)
        .iter()
        .find_map(|argv| match judge(argv) {
            Some(Judged::Refused(reason)) => Some(reason),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A state root holding the record real agent-desktop 1.0.2 wrote for Calculator, as
    /// snapshot `real`, and the same record relabelled as another app, as `term`.
    fn root(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yi-desktop-{}-{name}", std::process::id()));
        let real = include_str!("../tests/fixtures/agent-desktop-refmap.json");
        for (id, text) in [
            ("real", real.to_owned()),
            ("term", real.replace("\"Calculator\"", "\"Terminal\"")),
            (
                "chrome",
                real.replace("\"Calculator\"", "\"Google Chrome\""),
            ),
            ("blank", real.replace("\"Calculator\"", "null")),
        ] {
            let dir = root.join("snapshots").join(id);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("refmap.json"), text)?;
        }
        Ok(root)
    }

    fn judged(root: &Path, words: &[&str]) -> Option<Judged> {
        let argv: Vec<String> = std::iter::once(PROGRAM)
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect();
        judge_at(&argv, root)
    }

    fn app(name: &str) -> Option<Judged> {
        Some(Judged::App(name.to_owned()))
    }

    #[test]
    fn a_ref_is_judged_by_the_app_its_real_snapshot_record_names() -> TestResult {
        let root = root("record")?;
        assert_eq!(judged(&root, &["click", "@real:e7"]), app("calculator"));
        assert_eq!(
            judged(&root, &["type", "@real:e2", "hello"]),
            app("calculator")
        );
        assert_eq!(
            judged(&root, &["snapshot", "--root", "@real:e3", "-i"]),
            app("calculator")
        );
        Ok(())
    }

    #[test]
    fn a_ref_into_a_terminal_never_gets_a_standing_yes() -> TestResult {
        let root = root("terminal")?;
        assert_eq!(
            judged(&root, &["type", "@term:e2", "rm -rf ~"]),
            Some(Judged::EachTime)
        );
        Ok(())
    }

    #[test]
    fn two_apps_in_one_command_cannot_ride_one_grant() -> TestResult {
        let root = root("mixed")?;
        for words in [
            &["snapshot", "--app", "calculator", "--root", "@term:e3"][..],
            &["snapshot", "--app", "calculator", "--root=@term:e3"][..],
        ] {
            assert_eq!(judged(&root, words), Some(Judged::EachTime), "{words:?}");
        }
        Ok(())
    }

    #[test]
    fn a_record_that_cannot_name_the_app_asks_every_time() -> TestResult {
        let root = root("unknown")?;
        for reference in [
            "@real:e9999",
            "@gone:e1",
            "@blank:e7",
            "@../snapshots/real:e7",
            "@real",
            "@:e7",
        ] {
            assert_eq!(
                judged(&root, &["click", reference]),
                Some(Judged::EachTime),
                "{reference}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_browser_is_refused_by_name_or_by_ref() -> TestResult {
        let root = root("browser")?;
        for words in [
            &["snapshot", "--app", "Safari"][..],
            &["snapshot", "--app=Google Chrome"][..],
            &["launch", "firefox"][..],
            &["click", "@chrome:e7"][..],
        ] {
            assert!(
                matches!(judged(&root, words), Some(Judged::Refused(_))),
                "{words:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_named_app_is_scoped_whatever_its_case() {
        let root = Path::new("/nonexistent");
        assert_eq!(
            judged(root, &["snapshot", "--app", "TextEdit", "-i"]),
            app("textedit")
        );
        assert_eq!(
            judged(root, &["snapshot", "--app=TextEdit"]),
            app("textedit")
        );
        assert_eq!(judged(root, &["close-app", "textedit"]), app("textedit"));
    }

    #[test]
    fn every_spelling_the_lists_cannot_judge_asks_every_time() {
        let root = Path::new("/nonexistent");
        for words in [
            // Upstream takes a bundle id for --app, and this one is Terminal.
            &["snapshot", "--app", "com.apple.Terminal"][..],
            &["launch", "/Applications/Utilities/Terminal.app"][..],
            &["snapshot", "--app", "Terminal"][..],
            &["launch", "GHOSTTY"][..],
            &["snapshot"][..],
            &["snapshot", "--app", "TextEdit", "--surface", "focused"][..],
            &["snapshot", "--app", "TextEdit", "--surface=focused"][..],
            &["snapshot", "--app", "TextEdit", "--trace", "/tmp/x"][..],
            &["snapshot", "--app", "TextEdit", "--session", "../other"][..],
            &[
                "launch",
                "TextEdit",
                "--env",
                "DYLD_INSERT_LIBRARIES=/tmp/x",
            ][..],
            &["close-app", "TextEdit", "--force"][..],
            &["screenshot", "--app", "TextEdit", "/tmp/x.png"][..],
            &["clipboard-get"][..],
            &["permissions", "--guide"][..],
            &["a-verb-from-a-later-version"][..],
            &[][..],
        ] {
            assert_eq!(judged(root, words), Some(Judged::EachTime), "{words:?}");
        }
    }

    #[test]
    fn whatever_takes_the_users_own_input_is_refused() -> TestResult {
        let root = root("physical")?;
        for words in [
            &["--headed", "click", "@real:e7"][..],
            &["click", "@real:e7", "--headed"][..],
            &["-v", "--session", "x", "press", "cmd+q"][..],
            &["launch", "Calculator", "--activate"][..],
            &["press", "cmd+q"][..],
            &["hover", "@real:e7"][..],
            &["drag", "--from-xy", "1,2", "--to-xy", "3,4"][..],
            &["mouse-click", "--xy", "1,2"][..],
            &["double-click", "@real:e7"][..],
            &["batch", "[]"][..],
        ] {
            assert!(
                matches!(judged(&root, words), Some(Judged::Refused(_))),
                "{words:?}"
            );
        }
        // A word typed into an app is text, not a verb.
        assert_eq!(
            judged(&root, &["type", "@real:e2", "press"]),
            app("calculator")
        );
        Ok(())
    }

    #[test]
    fn the_agent_cursor_needs_no_question_and_its_session_keeps_the_scope() -> TestResult {
        let root = root("session")?;
        let held = root.join("sessions/run-1-2-0/snapshots/inner");
        std::fs::create_dir_all(&held)?;
        std::fs::copy(
            root.join("snapshots/real/refmap.json"),
            held.join("refmap.json"),
        )?;
        assert_eq!(
            judged(&root, &["session", "start", "--cursor"]),
            Some(Judged::Free)
        );
        assert_eq!(
            judged(&root, &["session", "end", "--session", "run-1-2-0"]),
            Some(Judged::Free)
        );
        assert_eq!(
            judged(&root, &["click", "@inner:e7", "--session", "run-1-2-0"]),
            app("calculator")
        );
        assert_eq!(
            judged(
                &root,
                &["snapshot", "--app", "TextEdit", "--session=run-1-2-0"]
            ),
            app("textedit")
        );
        assert_eq!(
            judged(&root, &["click", "@inner:e7"]),
            Some(Judged::EachTime)
        );
        Ok(())
    }

    #[test]
    fn only_the_plain_program_name_is_judged() {
        let root = Path::new("/nonexistent");
        for argv in [
            vec!["./agent-desktop", "version"],
            vec!["/tmp/agent-desktop", "version"],
            vec!["AGENT_DESKTOP_HOME=/tmp/forged", "agent-desktop", "version"],
            vec!["timeout", "5", "agent-desktop", "version"],
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
            assert_eq!(judge_at(&argv, root), None, "{argv:?}");
        }
    }

    #[test]
    fn a_free_verb_with_a_flag_is_not_free() {
        let root = Path::new("/nonexistent");
        assert_eq!(judged(root, &["status"]), Some(Judged::Free));
        assert_eq!(
            judged(root, &["skills", "get", "desktop"]),
            Some(Judged::Free)
        );
        assert_eq!(
            judged(root, &["permissions", "--request"]),
            Some(Judged::EachTime)
        );
    }
}
