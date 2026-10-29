use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use serde_json::{Value, json};

const PROVIDERS: [(&str, &str); 3] = [
    ("anthropic", "anthropic/claude-sonnet-5"),
    ("openai", "openai/gpt-5.5"),
    ("openrouter", "openrouter/anthropic/claude-sonnet-5"),
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
    let path = yi_runtime::settings::config_path(home);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            yi_types::config::parse(&raw).map_err(|error| {
                format!(
                    "{}: {error}; fix it, then run `yi setup` again",
                    path.display()
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    yi_runtime::set_catalog_cache_dir(home.join(".yi/catalog"));
    let mut edits: Vec<Edit> = Vec::new();
    let provider = ask(
        input,
        out,
        "Provider for the main model (anthropic, openai, openrouter; blank skips): ",
    )?;
    if !provider.is_empty() {
        model_step(home, &provider, input, out, &mut edits)?;
    }
    let mode = ask(
        input,
        out,
        "Permission mode: auto asks only for what Yi cannot prove safe, ask asks before every call, yolo never asks (blank skips): ",
    )?;
    match mode.to_lowercase().as_str() {
        "" => {}
        "auto" | "ask" | "yolo" => {
            edits.push((vec!["permissions", "mode"], json!(mode.to_lowercase())));
        }
        other => say(
            out,
            &format!("unknown mode {other}; the mode is left as it is"),
        )?,
    }
    let classifier = ask(
        input,
        out,
        "Turn on the optional classifier? It suggests skills from what you mean and approves ordinary commands in auto mode; typed messages and commands go to TypeSafe's Jev through OpenRouter, about $0.00002 a call. [y/N]: ",
    )?;
    if yes(&classifier) {
        classifier_step(out, &mut edits)?;
    }
    if edits.is_empty() {
        return say(out, "Nothing to save.");
    }
    let path = yi_runtime::settings::write(home, &edits)?;
    say(out, &format!("Saved {}.", path.display()))
}

fn model_step(
    home: &Path,
    provider: &str,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let provider = provider.to_lowercase();
    let Some((name, default)) = PROVIDERS.iter().find(|(name, _)| *name == provider) else {
        return say(
            out,
            &format!("unknown provider {provider}; the model is left as it is"),
        );
    };
    if yi_runtime::auth::api_key(name).is_none() {
        say(out, &yi_runtime::auth::missing_message(name))?;
    }
    let answer = ask(input, out, &format!("Model [{default}]: "))?;
    let model = if answer.is_empty() {
        (*default).to_owned()
    } else {
        answer
    };
    if crate::resolve(&model).is_none() {
        return say(
            out,
            &format!("unknown model {model}; the model is left as it is"),
        );
    }
    let primary = std::fs::read_to_string(yi_runtime::settings::config_path(home))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .is_some_and(|config| config.pointer("/models/primary").is_some());
    let key = if primary {
        vec!["models", "primary"]
    } else {
        vec!["model"]
    };
    edits.push((key, json!(model)));
    Ok(())
}

fn classifier_step(out: &mut dyn Write, edits: &mut Vec<Edit>) -> Result<(), String> {
    if yi_runtime::auth::api_key("openrouter").is_none() {
        return say(
            out,
            &format!(
                "{}; the classifier is not saved",
                yi_runtime::auth::missing_message("openrouter")
            ),
        );
    }
    edits.push((
        vec!["models", "classifier"],
        json!(yi_runtime::classifier::JEV),
    ));
    edits.push((vec!["classifier", "url"], Value::Null));
    Ok(())
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

fn yes(answer: &str) -> bool {
    matches!(answer.to_lowercase().as_str(), "y" | "yes")
}

fn yes_default(answer: &str) -> bool {
    answer.is_empty() || yes(answer)
}

#[cfg(test)]
mod tests {
    use super::offers;

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
