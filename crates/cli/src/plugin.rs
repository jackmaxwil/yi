use std::path::Path;

use yi_runtime::plugin::{self, PLUGINS, Plugin, State};

const USAGE: &str = "usage: yi plugin list | add <name>|--all | remove <name> | sync";

pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() != Some("plugin") {
        return;
    }
    let home = crate::home();
    if home.as_os_str().is_empty() {
        eprintln!("error: HOME is not set; yi needs an absolute HOME");
        std::process::exit(2);
    }
    if !home.is_absolute() {
        eprintln!(
            "error: HOME is relative ({}); yi needs an absolute HOME",
            home.display()
        );
        std::process::exit(2);
    }
    let args: Vec<String> = std::env::args().skip(2).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    std::process::exit(run(home, &words));
}

fn run(home: &Path, words: &[&str]) -> i32 {
    let picked = match words {
        ["list"] | [] => return list(home),
        ["add", "--all"] => PLUGINS.iter().collect(),
        ["sync"] => PLUGINS
            .iter()
            .filter(|plugin| plugin::chosen(plugin, home))
            .collect(),
        ["add" | "remove", name] if !name.starts_with('-') => match Plugin::named(name) {
            Some(plugin) => vec![plugin],
            None => {
                eprintln!("error: no plugin named {name}; `yi plugin list` names them");
                return 2;
            }
        },
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    if picked.is_empty() {
        println!("nothing to sync: yi installed no plugin here");
        return 0;
    }
    let mut code = 0;
    for plugin in picked {
        let done = if words.first() == Some(&"remove") {
            plugin::remove(plugin, home).map(|removed| match removed {
                true => format!("{} removed", plugin.name),
                false => format!("{} was not installed by yi", plugin.name),
            })
        } else {
            add(plugin, home)
        };
        match done {
            Ok(line) => println!("{line}"),
            Err(error) => {
                eprintln!("error: {error}");
                code = 1;
            }
        }
    }
    code
}

fn add(plugin: &Plugin, home: &Path) -> Result<String, String> {
    if plugin.installed(home).is_none() {
        eprintln!("downloading {} {}", plugin.name, plugin.version);
    }
    let path = plugin::add(plugin, home, plugin::fetch)?;
    Ok(format!(
        "{} {} installed at {}",
        plugin.name,
        plugin.version,
        path.display()
    ))
}

fn list(home: &Path) -> i32 {
    for plugin in &PLUGINS {
        let state = match plugin::state(plugin, home) {
            State::Installed => "installed".to_owned(),
            State::Stale(versions) => format!(
                "{}: yi plugin sync",
                plugin::stale_text(&versions, plugin.version)
            ),
            State::OnPath(path) => format!(
                "on PATH at {}, a version yi did not install: yi plugin add {}",
                path.display(),
                plugin.name
            ),
            State::Missing => format!("not installed: yi plugin add {}", plugin.name),
            State::Unsupported => plugin::unsupported(plugin),
        };
        println!("{} {}  {state}", plugin.name, plugin.version);
        println!("  {}", plugin.summary);
    }
    0
}
