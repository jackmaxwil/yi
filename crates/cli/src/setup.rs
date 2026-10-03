use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

const PROVIDERS: [(&str, &str); 3] = [
    ("anthropic", "anthropic/claude-sonnet-5"),
    ("openai", "openai/gpt-5.5"),
    ("openrouter", "openrouter/anthropic/claude-sonnet-5"),
];
pub(crate) const LAYA_VENV: &str = ".local/share/laya-venv";
const CLASSIFIER_URL: &str = "http://127.0.0.1:8000";
const CHECKPOINT: &str = "english";

type Edit = (Vec<&'static str>, Value);

pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() == Some("setup") {
        std::process::exit(run());
    }
    let home = crate::home();
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !offers(
        std::env::args().nth(1).as_deref(),
        config_path(home).symlink_metadata().is_ok(),
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
        if let Err(error) = write_config(home, &[]) {
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

fn config_path(home: &Path) -> PathBuf {
    home.join(".yi/config.json")
}

fn flow(home: &Path, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), String> {
    let path = config_path(home);
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
        "Set up the optional skill classifier? It suggests skills from what you mean, not only trigger words, with a local 808 MB model (about 100 ms a message on CPU). [y/N]: ",
    )?;
    if yes(&classifier) {
        classifier_step(home, input, out, &mut edits)?;
    }
    if edits.is_empty() {
        return say(out, "Nothing to save.");
    }
    let path = write_config(home, &edits)?;
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
    let primary = std::fs::read_to_string(config_path(home))
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

fn classifier_step(
    home: &Path,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let venv = home.join(LAYA_VENV);
    let install = ask(
        input,
        out,
        &format!(
            "Install laya-serve into {} now? It downloads about 1-2 GB. [y/N]: ",
            venv.display()
        ),
    )?;
    if yes(&install)
        && let Err(error) = install_laya(&venv)
    {
        say(out, &format!("laya-serve did not install: {error}"))?;
    }
    say(
        out,
        &format!(
            "`yi serve` starts it once this is saved; to probe it now, start it in another terminal:\n  LAYA_HOST=127.0.0.1 LAYA_PORT=8000 {}",
            venv.join("bin/laya-serve").display()
        ),
    )?;
    let answer = ask(
        input,
        out,
        &format!("Its URL once running [{CLASSIFIER_URL}]: "),
    )?;
    let url = if answer.is_empty() {
        CLASSIFIER_URL.to_owned()
    } else {
        answer
    };
    if let Err(error) = yi_runtime::classifier::probe(&url, CHECKPOINT) {
        return say(
            out,
            &format!(
                "no classifier answered at {url} ({error}); nothing saved for it, so run `yi setup` again once it runs"
            ),
        );
    }
    edits.push((vec!["models", "classifier"], json!(CHECKPOINT)));
    edits.push((vec!["classifier", "url"], json!(url)));
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "setup installs laya-serve only after the user says yes"
)]
fn install_laya(venv: &Path) -> Result<(), String> {
    let python = venv.join("bin/python");
    let uv = std::process::Command::new("uv")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let steps: [Vec<std::ffi::OsString>; 2] = if uv {
        [
            vec!["uv".into(), "venv".into(), venv.into()],
            vec![
                "uv".into(),
                "pip".into(),
                "install".into(),
                "--python".into(),
                python.into(),
                "laya[serve]==0.3.21".into(),
            ],
        ]
    } else {
        [
            vec!["python3".into(), "-m".into(), "venv".into(), venv.into()],
            vec![
                venv.join("bin/pip").into(),
                "install".into(),
                "laya[serve]==0.3.21".into(),
            ],
        ]
    };
    for step in steps {
        let (program, args) = step.split_first().ok_or("an empty install step")?;
        let status = std::process::Command::new(program)
            .args(args)
            .status()
            .map_err(|error| format!("{}: {error}", program.to_string_lossy()))?;
        if !status.success() {
            return Err(format!("{} exited {status}", program.to_string_lossy()));
        }
    }
    Ok(())
}

fn write_config(home: &Path, edits: &[Edit]) -> Result<PathBuf, String> {
    let path = config_path(home);
    let mut root = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("{}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    for (keys, value) in edits {
        set(&mut root, keys, value.clone())?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())? + "\n";
    yi_types::config::parse(&text)
        .map_err(|error| format!("setup would write a config Yi cannot load: {error}"))?;
    let target = match (std::fs::canonicalize(&path), std::fs::read_link(&path)) {
        (Ok(target), _) => target,
        (Err(_), Ok(link)) => path.parent().ok_or("config path has no parent")?.join(link),
        (Err(_), Err(_)) => path.clone(),
    };
    let dir = target.parent().ok_or("config path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mode = std::fs::metadata(&target).map_or(0o600, |meta| {
        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777
    });
    let staging = dir.join(".config.json.setup");
    let written = stage(&staging, &text, mode)
        .and_then(|()| std::fs::rename(&staging, &target))
        .map_err(|error| format!("{}: {error}", target.display()));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written.map(|()| path)
}

fn stage(staging: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(staging)?;
    std::fs::set_permissions(staging, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn set(root: &mut Value, keys: &[&str], value: Value) -> Result<(), String> {
    let (last, parents) = keys.split_last().ok_or("an empty key path")?;
    let mut node = root;
    for key in parents {
        node = node
            .as_object_mut()
            .ok_or_else(|| format!("`{key}` sits under a value that is not an object"))?
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    node.as_object_mut()
        .ok_or_else(|| format!("`{last}` sits under a value that is not an object"))?
        .insert((*last).to_owned(), value);
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
