use std::path::{Path, PathBuf};

use yi_runtime::memory::{Note, Store, block, global_dir, ranked, repo_dir};

const USAGE: &str = "usage: yi memory [list | show <name> | search <words> | forget <name> | import <dir> | stats | check | rebuild]";

fn find<'a>(
    stores: &'a [(&'static str, Store)],
    query: &str,
) -> Option<(&'static str, &'a Store, Note)> {
    stores
        .iter()
        .find_map(|(label, store)| store.resolve(query).map(|note| (*label, store, note)))
}

fn list(home: &Path, cwd: &Path, stores: &[(&'static str, Store)]) -> i32 {
    let (_, summary) = block(home, cwd);
    println!("  {}", summary.line());
    for (label, store) in stores {
        for note in store.notes() {
            let kind = note.kind.map_or("-", |kind| kind.as_str());
            let mark = if note.trouble.is_some() { "!" } else { " " };
            println!(
                "{mark} {:<30} {kind:<9} {label:<6} {}",
                note.name, note.hook
            );
        }
    }
    for (label, store) in stores {
        println!("  {label}: {}", store.dir().display());
    }
    0
}

fn tenths(count: u64, sessions: u64) -> String {
    let scaled = count.saturating_mul(10).checked_div(sessions).unwrap_or(0);
    format!("{}.{}", scaled / 10, scaled % 10)
}

fn stats(stores: &[(&'static str, Store)]) -> i32 {
    for (label, store) in stores {
        let usage = store.usage();
        let saves: u64 = usage.notes.values().map(|entry| entry.saves).sum();
        let reads: u64 = usage.notes.values().map(|entry| entry.reads).sum();
        let unparsed = store
            .notes()
            .iter()
            .filter(|note| note.trouble.is_some())
            .count();
        println!(
            "  {label} · sessions {} · saves {saves} ({}/session) · reads {reads} ({}/session) · {unparsed} unparsed",
            usage.sessions,
            tenths(saves, usage.sessions),
            tenths(reads, usage.sessions),
        );
    }
    0
}

fn check(stores: &[(&'static str, Store)]) -> i32 {
    let mut troubled = 0usize;
    for (_, store) in stores {
        for note in store.notes() {
            if let Some(trouble) = &note.trouble {
                troubled = troubled.saturating_add(1);
                let path = store.dir().join(note.name.file());
                println!("{}:{}: {}", path.display(), trouble.line, trouble.reason);
            }
        }
    }
    if troubled == 0 {
        println!("memory · every note parses");
        0
    } else {
        1
    }
}

fn rebuild(stores: &[(&'static str, Store)]) -> i32 {
    let mut code = 0;
    for (label, store) in stores {
        let report = match store.rebuild() {
            Ok(report) => report,
            Err(error) => {
                eprintln!("error: {label}: {error}");
                code = 1;
                continue;
            }
        };
        let journal = report.broken.map_or_else(
            || "journal ok".to_owned(),
            |line| format!("journal breaks at line {line}"),
        );
        println!(
            "  {label} · {} notes · {} restored · {} forgotten · {journal}",
            report.live, report.restored, report.forgotten
        );
        for name in &report.differ {
            println!(
                "  {label} · {name}: the file differs from the journal; a session start records it as an edit"
            );
        }
        for name in &report.missing {
            println!("  {label} · {name}: no object holds this version");
            code = 1;
        }
        if report.broken.is_some() {
            code = 1;
        }
    }
    code
}

pub fn run(args: &crate::Args) -> i32 {
    let cwd = crate::effective_cwd(args);
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let stores = [
        ("repo", Store::new(repo_dir(&home, &cwd))),
        ("global", Store::new(global_dir(&home))),
    ];
    let line = args.prompt.trim();
    let (verb, rest) = line
        .split_once(' ')
        .map_or((line, ""), |(verb, rest)| (verb, rest.trim()));
    match (verb, rest) {
        ("" | "list", "") => list(&home, &cwd, &stores),
        ("stats", "") => stats(&stores),
        ("check", "") => check(&stores),
        ("rebuild", "") => rebuild(&stores),
        ("search", query) if !query.is_empty() => {
            let refs: Vec<&Store> = stores.iter().map(|(_, store)| store).collect();
            for (at, note) in ranked(&refs, query) {
                let label = stores.get(at).map_or("", |(label, _)| *label);
                println!("{:<30} {label:<6} {}", note.name, note.hook);
            }
            0
        }
        ("show", query) if !query.is_empty() => match find(&stores, query) {
            Some((_, store, note)) => {
                match std::fs::read_to_string(store.dir().join(note.name.file())) {
                    Ok(text) => {
                        print!("{text}");
                        0
                    }
                    Err(error) => {
                        eprintln!("error: {}: {error}", note.name);
                        1
                    }
                }
            }
            None => {
                eprintln!("error: no note matches {query:?} by name or hook");
                1
            }
        },
        ("forget", query) if !query.is_empty() => match find(&stores, query) {
            Some((label, store, note)) => match store.forget(&note.name) {
                Ok(_) => {
                    println!("memory · forgot {} · {label}", note.name);
                    0
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    1
                }
            },
            None => {
                eprintln!("error: no note matches {query:?} by name or hook");
                1
            }
        },
        ("import", dir) if !dir.is_empty() => {
            let Some((_, repo)) = stores.first() else {
                return 1;
            };
            match repo.import(Path::new(dir)) {
                Ok(report) => {
                    println!(
                        "memory · imported {} · updated {} · skipped {}",
                        report.imported, report.updated, report.skipped
                    );
                    0
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    1
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}
