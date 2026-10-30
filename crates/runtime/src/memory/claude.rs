//! The Claude Code memories of this working directory, imported as Yi notes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::doc::{MemoryName, Note, source_of};
use super::store::{
    Imported, LOAD_BYTES, LOAD_LINES, Store, StoreError, global_dir, loaded, repo_dir,
};

/// The frontmatter key every imported note carries, so a note Claude Code wrote is told from one
/// Yi wrote.
pub const SOURCE: &str = "claude";

fn encoded(home: &Path, cwd: &Path) -> PathBuf {
    let key: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(key).join("memory")
}

/// `~/.claude/projects/<the working directory, every byte but an alphanumeric a dash>/memory`,
/// keyed by the path as Claude Code was handed it or, when only that one exists, by its real path.
pub fn project_dir(home: &Path, cwd: &Path) -> PathBuf {
    let literal = encoded(home, cwd);
    if literal.is_dir() {
        return literal;
    }
    match cwd.canonicalize() {
        Ok(real) => {
            let resolved = encoded(home, &real);
            if resolved.is_dir() { resolved } else { literal }
        }
        Err(_) => literal,
    }
}

/// `~/.claude/memory`, the notes Claude Code keeps for every project.
pub fn user_dir(home: &Path) -> PathBuf {
    home.join(".claude/memory")
}

/// `memory.importClaude`: on unless the config says false, and an unreadable config imports.
pub fn enabled(home: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(home.join(".yi/config.json")) else {
        return true;
    };
    let Ok((config, _)) = yi_types::config::parse(&raw) else {
        return true;
    };
    config
        .memory
        .and_then(|memory| memory.import_claude)
        .unwrap_or(true)
}

fn names_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name();
            let stem = file.to_str()?.strip_suffix(".md")?;
            MemoryName::from_stem(stem).map(|name| name.as_str().to_owned())
        })
        .collect()
}

fn short(home: &Path, path: &Path) -> String {
    path.strip_prefix(home).map_or_else(
        |_| path.display().to_string(),
        |rest| format!("~/{}", rest.display()),
    )
}

/// The Claude Code memory directories this session reads, and every note name any of them holds.
/// One store is shared by every worktree of a repository, so a note from another is not stale here.
#[derive(Debug, Clone)]
pub struct Sources {
    project: PathBuf,
    user: PathBuf,
    names: BTreeSet<String>,
}

impl Sources {
    pub fn discover(home: &Path, cwd: &Path) -> Self {
        let project = project_dir(home, cwd);
        let user = user_dir(home);
        let mut dirs = vec![project.clone(), user.clone()];
        if let Ok(entries) = std::fs::read_dir(home.join(".claude/projects")) {
            for entry in entries.flatten() {
                let dir = entry.path().join("memory");
                if dir.is_dir() {
                    dirs.push(dir);
                }
            }
        }
        let names = dirs.iter().flat_map(|dir| names_in(dir)).collect();
        Self {
            project,
            user,
            names,
        }
    }

    pub fn project(&self) -> &Path {
        &self.project
    }

    pub fn user(&self) -> &Path {
        &self.user
    }

    /// An imported note whose source note is gone from every Claude Code directory.
    pub fn stale(&self, note: &Note) -> bool {
        source_of(note) == Some(SOURCE) && !self.names.contains(note.name.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub source: String,
    pub found: usize,
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub unreadable: usize,
    pub stale: usize,
    pub notice: Option<String>,
    pub error: Option<String>,
}

impl Report {
    pub fn line(&self) -> String {
        if let Some(error) = &self.error {
            return format!("claude · {} not imported: {error}", self.source);
        }
        let mut line = format!("claude · {} notes in {}", self.found, self.source);
        for (count, label) in [
            (self.imported, "imported"),
            (self.updated, "updated"),
            (self.unreadable, "unreadable"),
            (self.stale, "stale"),
        ] {
            if count > 0 {
                line.push_str(&format!(" · {count} {label}"));
            }
        }
        if let Some(notice) = &self.notice {
            line.push_str(&format!(" · {notice}"));
        }
        line
    }
}

fn one(sources: &Sources, home: &Path, store: &Store, dir: &Path) -> Result<Report, StoreError> {
    let found = names_in(dir).len();
    let Imported {
        imported,
        updated,
        skipped,
        unreadable,
    } = store.import(dir, Some(SOURCE))?;
    let index = store.indexed()?;
    let (shown, dropped) = loaded(&index, &store.usage());
    let stale = store
        .notes()
        .iter()
        .filter(|note| sources.stale(note))
        .count();
    let shown = shown.len();
    let notice = (dropped > 0 && imported.saturating_add(updated) > 0).then(|| {
        let (cap, limit) = if shown >= LOAD_LINES {
            ("LOAD_LINES", LOAD_LINES)
        } else {
            ("LOAD_BYTES", LOAD_BYTES)
        };
        crate::listed::cut_row(
            shown,
            shown.saturating_add(dropped),
            "notes loaded",
            cap,
            limit,
            "await memory.search(\"words\")",
        )
    });
    Ok(Report {
        source: short(home, dir),
        found,
        imported,
        updated,
        skipped,
        unreadable,
        stale,
        notice,
        error: None,
    })
}

/// Every Claude Code memory directory this session has, imported into the store that owns its
/// scope: the working directory's into the repository store, the user-level one into global.
pub fn run(home: &Path, cwd: &Path) -> Vec<Report> {
    let sources = Sources::discover(home, cwd);
    let mut out = Vec::new();
    for (store, dir) in [
        (
            Store::new(repo_dir(home, cwd)),
            sources.project().to_path_buf(),
        ),
        (Store::new(global_dir(home)), sources.user().to_path_buf()),
    ] {
        if !dir.is_dir() || names_in(&dir).is_empty() {
            continue;
        }
        out.push(match one(&sources, home, &store, &dir) {
            Ok(report) => report,
            Err(error) => Report {
                source: short(home, &dir),
                error: Some(error.to_string()),
                ..Report::default()
            },
        });
    }
    out
}

pub fn run_if_enabled(home: &Path, cwd: &Path) -> Vec<Report> {
    if enabled(home) {
        run(home, cwd)
    } else {
        Vec::new()
    }
}

/// What `yi doctor` reports: the working directory's directory found, imported and stale, read
/// without writing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    pub source: Option<String>,
    pub found: usize,
    pub imported: usize,
    pub stale: usize,
}

impl Tally {
    pub fn line(&self) -> String {
        let Some(source) = &self.source else {
            return "none for this directory".to_owned();
        };
        let mut line = format!(
            "{} notes in {source} · {} imported",
            self.found, self.imported
        );
        if self.stale > 0 {
            line.push_str(&format!(" · {} stale", self.stale));
        }
        line
    }
}

pub fn tally(home: &Path, cwd: &Path) -> Tally {
    let sources = Sources::discover(home, cwd);
    let dir = sources.project();
    if !dir.is_dir() {
        return Tally::default();
    }
    let notes = Store::new(repo_dir(home, cwd)).notes();
    Tally {
        source: Some(short(home, dir)),
        found: names_in(dir).len(),
        imported: notes
            .iter()
            .filter(|note| source_of(note) == Some(SOURCE))
            .count(),
        stale: notes.iter().filter(|note| sources.stale(note)).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::block_with;
    use crate::scratch::Scratch;
    use std::fs;

    const NOTE: &str =
        "---\nname: {name}\ndescription: {hook}\nmetadata:\n  type: project\n---\n\n{body}\n";

    fn site(label: &str) -> std::io::Result<(Scratch, PathBuf, PathBuf)> {
        let scratch = Scratch::new(&format!("yi-claude-{label}"))?;
        let home = scratch.join("home");
        let cwd = scratch.join("repo");
        fs::create_dir_all(&cwd)?;
        fs::create_dir_all(project_dir(&home, &cwd))?;
        Ok((scratch, home, cwd))
    }

    fn write_note(dir: &Path, name: &str, hook: &str, body: &str) -> std::io::Result<()> {
        let text = NOTE
            .replace("{name}", name)
            .replace("{hook}", hook)
            .replace("{body}", body);
        fs::write(dir.join(format!("{name}.md")), text)
    }

    fn repo(home: &Path, cwd: &Path) -> Store {
        Store::new(repo_dir(home, cwd))
    }

    #[test]
    fn the_working_directory_names_its_project_and_the_user_level_one_is_read() {
        let home = Path::new("/h");
        assert_eq!(
            project_dir(home, Path::new("/Users/jack/Dev/yi")),
            PathBuf::from("/h/.claude/projects/-Users-jack-Dev-yi/memory")
        );
        assert_eq!(user_dir(home), PathBuf::from("/h/.claude/memory"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_working_directory_finds_the_memory_claude_keyed_by_its_real_path() {
        let (_scratch, home, _cwd) = site("symlink").expect("scratch");
        let real = home.join("real");
        fs::create_dir_all(&real).expect("dir");
        let link = home.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let dir = encoded(&home, &fs::canonicalize(&real).expect("real"));
        fs::create_dir_all(&dir).expect("dir");
        write_note(&dir, "alpha", "a hook", "body").expect("note");
        assert_eq!(project_dir(&home, &link), dir);
        assert_eq!(run(&home, &link)[0].imported, 1);
    }

    #[test]
    fn a_first_import_tags_every_note_and_a_second_one_creates_nothing() {
        let (_scratch, home, cwd) = site("first").expect("scratch");
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "the first hook", "alpha body").expect("note");
        write_note(&dir, "beta", "the second hook", "beta body").expect("note");
        fs::write(
            dir.join("MEMORY.md"),
            "- [alpha](alpha.md) — the first hook\n",
        )
        .expect("index");

        let reports = run(&home, &cwd);
        assert_eq!(reports.len(), 1, "{reports:?}");
        let line = reports[0].line();
        assert!(
            line.starts_with("claude · 2 notes in ~/.claude/projects/"),
            "{line}"
        );
        assert!(line.ends_with("· 2 imported"), "{line}");
        let store = repo(&home, &cwd);
        assert_eq!(
            source_of(&store.resolve("alpha").expect("alpha")),
            Some("claude")
        );
        let index = fs::read_to_string(store.dir().join("MEMORY.md")).expect("index");
        assert!(
            index.contains("- [alpha](alpha.md) — the first hook"),
            "{index}"
        );
        assert!(
            index.contains("- [beta](beta.md) — the second hook"),
            "{index}"
        );

        let again = run(&home, &cwd);
        assert_eq!(
            (again[0].imported, again[0].updated, again[0].skipped),
            (0, 0, 2)
        );
        let summary = crate::memory::listed(&home, &cwd);
        assert_eq!((summary.repo, summary.stale), (2, 0));
    }

    #[test]
    fn a_changed_source_note_updates_its_yi_memory() {
        let (_scratch, home, cwd) = site("changed").expect("scratch");
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "the first hook", "alpha body").expect("note");
        run(&home, &cwd);
        write_note(&dir, "alpha", "the revised hook", "a revised body").expect("note");
        let reports = run(&home, &cwd);
        assert_eq!(
            (reports[0].imported, reports[0].updated, reports[0].skipped),
            (0, 1, 0)
        );
        let note = repo(&home, &cwd).resolve("alpha").expect("alpha");
        assert_eq!(note.hook, "the revised hook");
        assert!(note.body.contains("a revised body"), "{:?}", note.body);
    }

    #[test]
    fn a_note_deleted_at_the_source_is_marked_stale_and_kept() {
        let (_scratch, home, cwd) = site("stale").expect("scratch");
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "the first hook", "alpha body").expect("note");
        write_note(&dir, "beta", "the second hook", "beta body").expect("note");
        run(&home, &cwd);
        fs::remove_file(dir.join("alpha.md")).expect("remove");
        let reports = run(&home, &cwd);
        assert_eq!((reports[0].imported, reports[0].stale), (0, 1));
        let store = repo(&home, &cwd);
        assert!(
            store.dir().join("alpha.md").is_file(),
            "the note was deleted"
        );
        let (text, summary) = block_with(&home, &cwd, &Sources::discover(&home, &cwd));
        assert!(
            text.contains("- [alpha](alpha.md) — the first hook ⚠ stale"),
            "{text}"
        );
        assert_eq!(summary.stale, 1);
        assert!(!text.contains("beta.md) — the second hook ⚠"), "{text}");
    }

    #[test]
    fn one_note_that_does_not_parse_stops_neither_the_import_nor_the_index() {
        let (_scratch, home, cwd) = site("broken").expect("scratch");
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "the first hook", "alpha body").expect("note");
        fs::write(
            dir.join("broken-quote.md"),
            "---\nname: broken-quote\ndescription: \"never closed\n---\nIts first line.\n",
        )
        .expect("broken");
        write_note(&dir, "gamma", "the third hook", "gamma body").expect("note");
        let reports = run(&home, &cwd);
        assert_eq!((reports[0].found, reports[0].imported), (3, 3));
        let (text, summary) = block_with(&home, &cwd, &Sources::discover(&home, &cwd));
        assert!(
            text.contains("- [alpha](alpha.md) — the first hook"),
            "{text}"
        );
        assert!(
            text.contains("- [gamma](gamma.md) — the third hook"),
            "{text}"
        );
        assert!(
            text.contains("- [broken-quote](broken-quote.md) — Its first line."),
            "{text}"
        );
        assert_eq!(summary.unparsed, 1);
    }

    #[test]
    fn an_import_past_the_load_cap_names_the_cap_and_the_call_that_shows_the_rest() {
        let (_scratch, home, cwd) = site("cap").expect("scratch");
        let dir = project_dir(&home, &cwd);
        for i in 0..LOAD_LINES.saturating_add(2) {
            write_note(&dir, &format!("n{i}"), "a hook", "a body").expect("note");
        }
        let reports = run(&home, &cwd);
        assert_eq!(reports[0].imported, LOAD_LINES.saturating_add(2));
        assert_eq!(
            reports[0].notice.as_deref(),
            Some(
                "[… 200 of 202 notes loaded (LOAD_LINES 200); the rest: await memory.search(\"words\")]"
            )
        );
        assert!(
            reports[0].line().contains("202 notes in"),
            "{:?}",
            reports[0]
        );
    }

    #[test]
    fn the_user_level_directory_lands_in_the_global_store() {
        let (_scratch, home, cwd) = site("global").expect("scratch");
        fs::create_dir_all(user_dir(&home)).expect("dir");
        write_note(
            &user_dir(&home),
            "everywhere",
            "a hook for every repo",
            "body",
        )
        .expect("note");
        let reports = run(&home, &cwd);
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(
            reports[0].source.contains("~/.claude/memory"),
            "{:?}",
            reports[0]
        );
        assert_eq!(reports[0].imported, 1);
        let global = Store::new(global_dir(&home));
        assert_eq!(
            source_of(&global.resolve("everywhere").expect("note")),
            Some("claude")
        );
        assert_eq!(repo(&home, &cwd).notes().len(), 0);
    }

    #[test]
    fn claude_is_never_written_to() {
        let (_scratch, home, cwd) = site("readonly").expect("scratch");
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "the first hook", "alpha body").expect("note");
        fs::write(
            dir.join("MEMORY.md"),
            "- [alpha](alpha.md) — the first hook\n",
        )
        .expect("index");
        let snapshot = |dir: &Path| {
            let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
                .expect("dir")
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        fs::read(entry.path()).expect("read"),
                    )
                })
                .collect();
            files.sort();
            files
        };
        let before = snapshot(&dir);
        run(&home, &cwd);
        run(&home, &cwd);
        assert_eq!(snapshot(&dir), before);
    }

    #[test]
    fn the_switch_off_imports_nothing_and_the_verb_still_does() {
        let (_scratch, home, cwd) = site("switch").expect("scratch");
        fs::create_dir_all(home.join(".yi")).expect("home");
        fs::write(
            home.join(".yi/config.json"),
            r#"{"memory":{"importClaude":false}}"#,
        )
        .expect("config");
        write_note(&project_dir(&home, &cwd), "alpha", "a hook", "body").expect("note");
        assert!(!enabled(&home));
        assert!(run_if_enabled(&home, &cwd).is_empty());
        assert_eq!(repo(&home, &cwd).notes().len(), 0);
        assert_eq!(run(&home, &cwd)[0].imported, 1);
        assert!(enabled(
            &Scratch::new("yi-claude-default")
                .expect("scratch")
                .join("home")
        ));
    }

    #[test]
    fn the_tally_counts_what_doctor_reports_without_writing() {
        let (_scratch, home, cwd) = site("tally").expect("scratch");
        let empty = tally(&home, &cwd);
        assert_eq!((empty.found, empty.imported), (0, 0));
        let dir = project_dir(&home, &cwd);
        write_note(&dir, "alpha", "a hook", "body").expect("note");
        let one = tally(&home, &cwd);
        assert_eq!((one.found, one.imported), (1, 0));
        assert!(
            one.line().contains("1 notes in ~/.claude/projects/"),
            "{:?}",
            one.line()
        );
        assert_eq!(repo(&home, &cwd).notes().len(), 0, "the tally imported");
        run(&home, &cwd);
        fs::remove_file(dir.join("alpha.md")).expect("remove");
        let line = tally(&home, &cwd).line();
        assert!(line.contains("0 notes in"), "{line}");
        assert!(line.contains("1 imported · 1 stale"), "{line}");
    }
}
