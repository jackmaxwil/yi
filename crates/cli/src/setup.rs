use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use serde_json::{Value, json};
use yi_types::config::{ModeName, UserConfig};

#[rustfmt::skip]
const PROVIDERS: [(&str, &str, &str, &str); 4] = [
    ("anthropic", "anthropic/claude-sonnet-5", "Anthropic", "https://console.anthropic.com/settings/keys"),
    ("openai", "openai/gpt-5.5", "OpenAI", "https://platform.openai.com/api-keys"),
    ("openrouter", "openrouter/anthropic/claude-sonnet-5", "OpenRouter", "https://openrouter.ai/keys"),
    ("google", "google/gemini-2.5-pro", "Google", "https://aistudio.google.com/apikey"),
];

#[rustfmt::skip]
const MODES: [(ModeName, &str, &str); 3] = [
    (ModeName::Auto, "Smart (auto): asks only before something it cannot tell is safe", "auto"),
    (ModeName::Ask, "Careful (ask): asks before every action", "ask"),
    (ModeName::Yolo, "Hands-off (yolo): never asks, so only if you trust what it will do", "yolo"),
];

type Edit = (Vec<&'static str>, Value);

pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() == Some("setup") {
        std::process::exit(run());
    }
    let home = crate::home();
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let has_config = has_config(home);
    if !offers(std::env::args().nth(1).as_deref(), has_config, terminal) {
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
    if let Err(error) = flow(home, &mut input, &mut out, has_config) {
        eprintln!("warning: setup: {error}");
    }
}

#[rustfmt::skip]
fn run() -> i32 {
    let home = crate::home();
    match flow(home, &mut std::io::stdin().lock(), &mut std::io::stderr(), has_config(home)) {
        Ok(()) => 0, Err(error) => { eprintln!("error: {error}"); 1 }
    }
}

fn has_config(home: &Path) -> bool {
    yi_runtime::settings::config_path(home)
        .symlink_metadata()
        .is_ok()
}
fn offers(first_arg: Option<&str>, has_config: bool, terminal: bool) -> bool {
    first_arg.is_none() && !has_config && terminal
}

#[rustfmt::skip]
fn flow(home: &Path, input: &mut dyn BufRead, out: &mut dyn Write, has_config: bool) -> Result<(), String> {
    let (stored, _) = crate::user_config::read_config(home).map_err(|error| format!("{error}; fix it, then run `yi setup` again"))?;
    yi_runtime::set_catalog_cache_dir(home.join(".yi/catalog"));
    say(out, "Two quick questions. Type a number, or press Enter for the one marked [Enter].")?;
    let mut edits = Vec::new();
    model_step(&stored, input, out, &mut edits)?; mode_step(&stored, input, out, &mut edits)?;
    if edits.is_empty() {
        if !has_config { yi_runtime::settings::write(home, &[])?; }
        return say(out, "Nothing to change. Run `yi setup` any time.");
    }
    say(out, &format!("Saved {}. Run `yi` to start.", yi_runtime::settings::write(home, &edits)?.display()))
}

fn short(name: &str) -> &str {
    name.split_once(": ").map_or(name, |(_, rest)| rest)
}

#[rustfmt::skip]
fn candidates() -> Vec<(String, String, String, bool)> {
    let mut rows = Vec::new();
    for (provider, model, title, _) in PROVIDERS {
        let Some(found) = crate::resolve(model) else { continue };
        let has = yi_runtime::auth::api_key(provider).is_some();
        rows.push((provider.to_owned(), model.to_owned(), format!("{} on {title} ({})", short(&found.name), if has { "key found" } else { "needs a key" }), has));
    }
    let mut others: Vec<_> = yi_runtime::available_models().into_iter().filter(|found| PROVIDERS.iter().all(|(name, ..)| *name != found.provider)).filter(|found| yi_runtime::auth::api_key(&found.provider).is_some()).collect();
    others.sort_by(|a, b| (&a.provider, &b.id).cmp(&(&b.provider, &a.id))); others.dedup_by(|a, b| a.provider == b.provider);
    rows.extend(others.into_iter().map(|found| { let model = format!("{}/{}", found.provider, found.id); let label = format!("{} on {} (login found)", short(&found.name), found.provider); (found.provider, model, label, true) }));
    rows.sort_by_key(|row| !row.3); rows
}

#[rustfmt::skip]
fn model_step(stored: &UserConfig, input: &mut dyn BufRead, out: &mut dyn Write, edits: &mut Vec<Edit>) -> Result<(), String> {
    let roles = stored.models.as_ref().and_then(|models| models.primary.clone());
    let current = roles.clone().or_else(|| stored.model.clone()); let rows = candidates();
    let mut labels = Vec::new(); let mut picks = Vec::new();
    if let Some(model) = &current { labels.push(format!("Keep {model}")); picks.push(None); }
    for (index, row) in rows.iter().enumerate() { labels.push(row.2.clone()); picks.push(Some(index)); }
    if current.is_none() { labels.push("Decide later".to_owned()); picks.push(None); }
    let default = if current.is_some() || rows.first().is_some_and(|row| row.3) { 0 } else { labels.len().saturating_sub(1) };
    let Some(row) = picks.get(choose(input, out, "Which AI should Yi use?", &labels, None, default)?).copied().flatten().and_then(|index| rows.get(index)) else { return Ok(()) };
    if !row.3 && !key_step(&row.0, input, out)? { return Ok(()); }
    if current.as_ref() != Some(&row.1) { edits.push((if roles.is_some() { vec!["models", "primary"] } else { vec!["model"] }, json!(row.1))); }
    Ok(())
}

#[rustfmt::skip]
fn key_step(provider: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<bool, String> {
    let (title, url) = PROVIDERS.iter().find(|(name, ..)| *name == provider).map_or((provider, ""), |(_, _, title, url)| (*title, *url));
    if !url.is_empty() { say(out, &format!("Get a {title} key at {url}"))?; }
    let key = ask(input, out, &format!("API key for {provider}: "))?;
    if key.is_empty() { say(out, &yi_runtime::auth::missing_message(provider))?; return Ok(false); }
    if yi_runtime::auth::login(&[provider.to_owned(), "--key".to_owned(), key]) != 0 { say(out, &format!("The key was not saved; run `yi login {provider}` to try again."))?; }
    let found = yi_runtime::auth::api_key(provider).is_some(); if !found { say(out, &yi_runtime::auth::missing_message(provider))?; } Ok(found)
}

#[rustfmt::skip]
fn mode_step(stored: &UserConfig, input: &mut dyn BufRead, out: &mut dyn Write, edits: &mut Vec<Edit>) -> Result<(), String> {
    let current = stored.permissions.as_ref().and_then(|permissions| permissions.mode).unwrap_or(ModeName::Auto);
    let labels: Vec<_> = MODES.iter().map(|(_, label, _)| (*label).to_owned()).collect(); let answers: Vec<_> = MODES.iter().map(|(_, _, answer)| (*answer).to_owned()).collect();
    let default = MODES.iter().position(|(mode, _, _)| *mode == current).unwrap_or(0);
    if let Some((mode, _, _)) = MODES.get(choose(input, out, "How much should Yi ask before it acts?", &labels, Some(&answers), default)?).filter(|(mode, _, _)| *mode != current) { edits.push((vec!["permissions", "mode"], json!(mode))); }
    Ok(())
}

#[rustfmt::skip]
fn choose(input: &mut dyn BufRead, out: &mut dyn Write, title: &str, options: &[String], answers: Option<&[String]>, default: usize) -> Result<usize, String> {
    say(out, title)?; for (index, option) in options.iter().enumerate() { say(out, &format!("  {}) {option}{}", index.saturating_add(1), if index == default { " [Enter]" } else { "" }))?; }
    loop {
        let answer = ask(input, out, "Your choice: ")?; if answer.is_empty() { return Ok(default); }
        let by_number = answer.bytes().all(|byte| byte.is_ascii_digit()).then(|| answer.parse::<usize>().ok()).flatten().and_then(|number| number.checked_sub(1)).filter(|index| *index < options.len());
        let word = answer.to_lowercase(); let hits: Vec<_> = answers.unwrap_or(options).iter().enumerate().filter(|(_, option)| word.chars().count() >= 3 && option.to_lowercase().contains(&word)).map(|(index, _)| index).collect();
        match by_number.or(match hits.as_slice() { [only] => Some(*only), _ => None }) { Some(index) => return Ok(index), None if hits.len() > 1 => say(out, "More than one choice has that word; type its number.")?, None => say(out, &format!("Type a number from 1 to {}, or a word from the choice you want.", options.len()))?, }
    }
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
