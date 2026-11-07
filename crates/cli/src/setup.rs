use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use rustix::termios::{LocalModes, OptionalActions, Termios, tcgetattr, tcsetattr};
use serde_json::{Value, json};
use yi_types::config::{ModeName, UserConfig};

const PROVIDERS: [(&str, &str, &str, &str); 4] = [
    (
        "anthropic",
        "anthropic/claude-sonnet-5",
        "Anthropic",
        "https://console.anthropic.com/settings/keys",
    ),
    (
        "openai",
        "openai/gpt-5.5",
        "OpenAI",
        "https://platform.openai.com/api-keys",
    ),
    (
        "openrouter",
        "openrouter/anthropic/claude-sonnet-5",
        "OpenRouter",
        "https://openrouter.ai/keys",
    ),
    (
        "google",
        "google/gemini-2.5-pro",
        "Google",
        "https://aistudio.google.com/apikey",
    ),
];

const MODES: [(ModeName, &str); 3] = [
    (
        ModeName::Auto,
        "Smart (auto): asks only before something it cannot tell is safe",
    ),
    (ModeName::Ask, "Careful (ask): asks before every action"),
    (
        ModeName::Yolo,
        "Hands-off (yolo): never asks, so only if you trust what it will do",
    ),
];

type Edit = (Vec<&'static str>, Value);

pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() == Some("setup") {
        std::process::exit(run());
    }
    let home = crate::home();
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !offers(
        std::env::args().nth(1).as_deref(),
        yi_runtime::settings::config_path(home)
            .symlink_metadata()
            .is_ok(),
        terminal,
    ) {
        return;
    }
    let (mut input, mut out) = (std::io::stdin().lock(), std::io::stderr());
    let Ok(answer) = ask(
        &mut input,
        &mut out,
        "No ~/.yi/config.json yet. Set up Yi now? [Y/n]: ",
    ) else {
        return;
    };
    if !yes_default(&answer) {
        if let Err(error) = yi_runtime::settings::write(home, &[]) {
            eprintln!("warning: {error}");
        }
        eprintln!("Run `yi setup` any time.");
        return;
    }
    if let Err(error) = flow(home, &mut input, &mut out) {
        eprintln!("warning: setup: {error}");
    }
}

fn run() -> i32 {
    match flow(
        crate::home(),
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn offers(first_arg: Option<&str>, has_config: bool, terminal: bool) -> bool {
    first_arg.is_none() && !has_config && terminal
}

fn flow(home: &Path, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), String> {
    let (stored, _) = crate::user_config::read_config(home)
        .map_err(|error| format!("{error}; fix it, then run `yi setup` again"))?;
    yi_runtime::set_catalog_cache_dir(home.join(".yi/catalog"));
    say(
        out,
        "Two quick questions. Type a number, or press Enter for the one marked [Enter].",
    )?;
    let mut edits: Vec<Edit> = Vec::new();
    model_step(&stored, input, out, &mut edits)?;
    mode_step(&stored, input, out, &mut edits)?;
    if edits.is_empty() {
        if yi_runtime::settings::config_path(home)
            .symlink_metadata()
            .is_err()
        {
            yi_runtime::settings::write(home, &[])?;
        }
        return say(out, "Nothing to change. Run `yi setup` any time.");
    }
    let path = yi_runtime::settings::write(home, &edits)?;
    say(
        out,
        &format!("Saved {}. Run `yi` to start.", path.display()),
    )
}

/// OpenRouter names a model "Anthropic: Claude Sonnet 5"; the vendor prefix would make the word
/// "anthropic" match its row as well as Anthropic's own.
fn short(name: &str) -> &str {
    name.split_once(": ").map_or(name, |(_, rest)| rest)
}

/// Every model a person could pick, as (provider, model id, label, has credential): the four
/// API-key providers' flagships, then the other providers a stored login already unlocks.
fn candidates() -> Vec<(String, String, String, bool)> {
    let mut rows: Vec<(String, String, String, bool)> = Vec::new();
    for (provider, model, title, _) in PROVIDERS {
        let Some(found) = crate::resolve(model) else {
            continue;
        };
        let has = yi_runtime::auth::api_key(provider).is_some();
        let note = if has { "key found" } else { "needs a key" };
        let label = format!("{} on {title} ({note})", short(&found.name));
        rows.push((provider.to_owned(), model.to_owned(), label, has));
    }
    let mut others: Vec<yi_types::model::Model> = yi_runtime::available_models()
        .into_iter()
        .filter(|found| PROVIDERS.iter().all(|(name, ..)| *name != found.provider))
        .filter(|found| yi_runtime::auth::api_key(&found.provider).is_some())
        .collect();
    others.sort_by(|a, b| (&a.provider, &b.id).cmp(&(&b.provider, &a.id)));
    others.dedup_by(|a, b| a.provider == b.provider);
    for found in others {
        let model = format!("{}/{}", found.provider, found.id);
        let label = format!("{} on {} (login found)", short(&found.name), found.provider);
        rows.push((found.provider, model, label, true));
    }
    rows.sort_by_key(|row| !row.3);
    rows
}

fn model_step(
    stored: &UserConfig,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let roles = stored
        .models
        .as_ref()
        .and_then(|models| models.primary.clone());
    let current = roles.clone().or_else(|| stored.model.clone());
    let rows = candidates();
    let mut labels: Vec<String> = Vec::new();
    let mut picks: Vec<Option<usize>> = Vec::new();
    if let Some(model) = &current {
        labels.push(format!("Keep {model}"));
        picks.push(None);
    }
    for (index, row) in rows.iter().enumerate() {
        labels.push(row.2.clone());
        picks.push(Some(index));
    }
    if current.is_none() {
        labels.push("Decide later".to_owned());
        picks.push(None);
    }
    let default = if current.is_some() || rows.first().is_some_and(|row| row.3) {
        0
    } else {
        labels.len().saturating_sub(1)
    };
    let choice = choose(input, out, "Which AI should Yi use?", &labels, default)?;
    let Some(row) = picks
        .get(choice)
        .copied()
        .flatten()
        .and_then(|index| rows.get(index))
    else {
        return Ok(());
    };
    if !row.3 {
        key_step(&row.0, input, out)?;
    }
    if current.as_ref() != Some(&row.1) {
        let key = if roles.is_some() {
            vec!["models", "primary"]
        } else {
            vec!["model"]
        };
        edits.push((key, json!(row.1)));
    }
    Ok(())
}

/// Takes the key for a provider that has none, so a first-time user never has to leave setup
/// to run `yi login`; a blank answer leaves it for later and says how.
fn key_step(provider: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), String> {
    let (title, url) = PROVIDERS
        .iter()
        .find(|(name, ..)| *name == provider)
        .map_or((provider, ""), |(_, _, title, url)| (*title, *url));
    if !url.is_empty() {
        say(out, &format!("Get a {title} key at {url}"))?;
    }
    let prompt =
        format!("Paste your {title} key here (it stays hidden), or press Enter to add it later: ");
    let key = ask_secret(input, out, &prompt)?;
    if key.is_empty() {
        return say(out, &yi_runtime::auth::missing_message(provider));
    }
    let login = [provider.to_owned(), "--key".to_owned(), key];
    if yi_runtime::auth::login(&login) == 0 {
        return Ok(());
    }
    say(
        out,
        &format!("The key was not saved; run `yi login {provider}` to try again."),
    )
}

fn mode_step(
    stored: &UserConfig,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let current = (stored.permissions.as_ref())
        .and_then(|permissions| permissions.mode)
        .unwrap_or(ModeName::Auto);
    let labels: Vec<String> = MODES.iter().map(|(_, label)| (*label).to_owned()).collect();
    let default = MODES
        .iter()
        .position(|(mode, _)| *mode == current)
        .unwrap_or(0);
    let title = "How much should Yi ask before it acts?";
    let choice = choose(input, out, title, &labels, default)?;
    if let Some((mode, _)) = MODES.get(choice).filter(|(mode, _)| *mode != current) {
        edits.push((vec!["permissions", "mode"], json!(mode)));
    }
    Ok(())
}

/// A numbered menu; Enter takes `default`, a number picks that row, and a word of three
/// letters or more picks the one row containing it, so "anthropic" works as well as "1".
fn choose(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    title: &str,
    options: &[String],
    default: usize,
) -> Result<usize, String> {
    say(out, title)?;
    for (index, option) in options.iter().enumerate() {
        let mark = if index == default { " [Enter]" } else { "" };
        say(
            out,
            &format!("  {}) {option}{mark}", index.saturating_add(1)),
        )?;
    }
    loop {
        let answer = ask(input, out, "Your choice: ")?;
        if answer.is_empty() {
            return Ok(default);
        }
        let by_number = (answer.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| answer.parse::<usize>().ok())
            .flatten()
            .and_then(|number| number.checked_sub(1))
            .filter(|index| *index < options.len());
        let word = answer.to_lowercase();
        let hits: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, option)| {
                word.chars().count() >= 3 && option.to_lowercase().contains(&word)
            })
            .map(|(index, _)| index)
            .collect();
        let by_word = match hits.as_slice() {
            [only] => Some(*only),
            _ => None,
        };
        match by_number.or(by_word) {
            Some(index) => return Ok(index),
            None if hits.len() > 1 => {
                say(out, "More than one choice has that word; type its number.")?
            }
            None => say(
                out,
                &format!(
                    "Type a number from 1 to {}, or a word from the choice you want.",
                    options.len()
                ),
            )?,
        }
    }
}

/// Reads a line with the terminal's echo off when there is a terminal, so a pasted key is
/// not left in the scrollback.
fn ask_secret(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &str,
) -> Result<String, String> {
    let stdin = std::io::stdin();
    let before = if stdin.is_terminal() {
        hide_echo(&stdin)
    } else {
        None
    };
    let answer = ask(input, out, prompt);
    if let Some(before) = before {
        if tcsetattr(&stdin, OptionalActions::Now, &before).is_err() {
            say(
                out,
                "\nwarning: the terminal's echo could not be restored; run `reset`",
            )?;
        }
        say(out, "")?;
    }
    answer
}

fn hide_echo(stdin: &std::io::Stdin) -> Option<Termios> {
    let before = tcgetattr(stdin).ok()?;
    let mut quiet = before.clone();
    quiet.local_modes.remove(LocalModes::ECHO);
    tcsetattr(stdin, OptionalActions::Now, &quiet).ok()?;
    Some(before)
}

fn ask(input: &mut dyn BufRead, out: &mut dyn Write, prompt: &str) -> Result<String, String> {
    write!(out, "{prompt}").map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())?;
    let mut line = String::new();
    let read = input
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if read == 0 {
        return Err("no answer (end of input); nothing saved".to_owned());
    }
    Ok(line.trim().to_owned())
}

fn say(out: &mut dyn Write, text: &str) -> Result<(), String> {
    writeln!(out, "{text}").map_err(|error| error.to_string())
}

fn yes_default(answer: &str) -> bool {
    matches!(answer.to_lowercase().as_str(), "" | "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::{choose, offers};

    #[test]
    fn a_mistyped_choice_is_asked_again_not_taken_as_a_skip() -> Result<(), String> {
        let options = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut out = Vec::new();
        let picked = choose(&mut "9\nb\n0\n2\n".as_bytes(), &mut out, "T:", &options, 2)?;
        assert_eq!(picked, 1, "only a number in range is an answer");
        let named = ["Claude on Anthropic".to_owned(), "GPT on OpenAI".to_owned()];
        let word = choose(&mut "gpt\n".as_bytes(), &mut out, "T:", &named, 0)?;
        assert_eq!(word, 1, "one word that names one row picks it");
        let blank = choose(&mut "\n".as_bytes(), &mut out, "T:", &options, 2)?;
        assert_eq!(blank, 2, "Enter takes the default");
        Ok(())
    }

    #[test]
    fn only_a_first_interactive_launch_is_offered_setup() {
        assert!(offers(None, false, true));
        assert!(
            !offers(Some("--version"), false, true),
            "only a bare `yi` is offered it"
        );
        assert!(!offers(Some("--yolo"), false, true));
        assert!(
            !offers(Some("ask"), false, true),
            "a subcommand is never interrupted"
        );
        assert!(
            !offers(None, true, true),
            "a config file means it already ran or was declined"
        );
        assert!(
            !offers(None, false, false),
            "scripts and pipes are never asked"
        );
    }
}
