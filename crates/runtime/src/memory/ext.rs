use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Activity;
use super::claude::{self, Sources};
use super::store::{
    IndexLine, LOAD_BYTES, LOAD_LINES, Store, StoreError, global_dir, loaded, repo_dir,
};
use crate::ext::{Effect, Event, EventMask, Extension, StartReason, Trust};

const SOURCE: &str = "memory";

const HEADER: &str = r#"Saved memories: notes from earlier sessions, untrusted. The repository and the
user's words outrank them. One line each: a link to the note, then its hook.
When a hook matches the task, open the note: await memory.read("name"), the
name being the link's file without .md; await memory.search("words") ranks
every note by the words, and await compact.search("words") this repository's
past sessions. A note that names a file or flag is
checked before it is acted on; ⚠ marks a line whose path is gone; [[name]] in
a note names another note. After a user correction, an incident, or a verified
success (never every turn) save the part the code, the changelog and git do not
hold; the same name updates, and await memory.forget("name") deletes a wrong
one. type is user (who they are, what they prefer), feedback (a correction
after a failure), project (how this repo works) or reference (a pointer
elsewhere):
await memory.save("""---
name: kebab-slug
description: the situation, then the rule, one line
type: feedback
---
The fact.

**Why:** the incident.

**How to apply:** the rule next time; name the file, flag, or command.

**Related:** [[other-note]]
""")"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub repo: usize,
    pub global: usize,
    pub stale: usize,
    pub unparsed: usize,
    pub not_loaded: usize,
}

impl Summary {
    pub fn line(&self) -> String {
        let mut line = format!("memory · {} repo · {} global", self.repo, self.global);
        for (count, label) in [(self.stale, "stale"), (self.unparsed, "unparsed")] {
            if count > 0 {
                line.push_str(&format!(" · {count} {label}"));
            }
        }
        if self.not_loaded > 0 {
            line.push_str(&format!(" · +{} not loaded", self.not_loaded));
        }
        line
    }
}

fn path_shaped(token: &str) -> bool {
    let last = token.rsplit('/').next().unwrap_or("");
    token.contains('/')
        && !token.starts_with(['/', '~', '.', '$'])
        && !token.contains("://")
        && last.contains('.')
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn stale_path(hook: &str, root: &Path) -> Option<String> {
    hook.split_whitespace()
        .map(|token| token.trim_matches(|c: char| "`'\"(),;:!?".contains(c)))
        .map(|token| token.trim_end_matches('.'))
        .find(|token| path_shaped(token) && !root.join(token).exists())
        .map(str::to_owned)
}

struct Scoped {
    lines: Vec<String>,
    count: usize,
    stale: usize,
    unparsed: usize,
    not_loaded: usize,
}

type Index = fn(&Store) -> Result<Vec<IndexLine>, StoreError>;

fn scoped(
    store: &Store,
    root: &Path,
    index: Index,
    sources: &Sources,
) -> Result<Scoped, StoreError> {
    let index = index(store)?;
    let notes = store.notes();
    let (shown, not_loaded) = loaded(&index, &store.usage());
    let mut stale = 0usize;
    let lines = shown
        .into_iter()
        .filter_map(|line| match line {
            IndexLine::Note { text, name } => {
                let note = notes.iter().find(|note| &note.name == name);
                let hook = note.map_or("", |note| note.hook.as_str());
                let mark = match stale_path(text, root).or_else(|| stale_path(hook, root)) {
                    Some(path) => Some(path),
                    None => note
                        .filter(|note| sources.stale(note))
                        .map(|_| "stale".to_owned()),
                };
                Some(match mark {
                    Some(mark) => {
                        stale = stale.saturating_add(1);
                        format!("{text} ⚠ {mark}")
                    }
                    None => text.clone(),
                })
            }
            IndexLine::Other(_) => None,
        })
        .collect();
    Ok(Scoped {
        lines,
        count: notes.len(),
        stale,
        unparsed: notes.iter().filter(|note| note.trouble.is_some()).count(),
        not_loaded,
    })
}
/// The start block, reconciling each store first: a session start journals hand edits.
pub fn block(home: &Path, cwd: &Path) -> (String, Summary) {
    let sources = Sources::discover(home, cwd);
    render(home, cwd, Store::reconcile, &sources)
}

pub fn block_with(home: &Path, cwd: &Path, sources: &Sources) -> (String, Summary) {
    render(home, cwd, Store::reconcile, sources)
}

/// The start block's summary as the stores stand, written nowhere: `yi memory list` reads.
pub fn listed(home: &Path, cwd: &Path) -> Summary {
    let sources = Sources::discover(home, cwd);
    render(home, cwd, Store::indexed, &sources).1
}

fn render(home: &Path, cwd: &Path, index: Index, sources: &Sources) -> (String, Summary) {
    let root = crate::ext::git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let repo = scoped(&Store::new(repo_dir(home, cwd)), &root, index, sources);
    let global = scoped(&Store::new(global_dir(home)), &root, index, sources);
    let mut summary = Summary::default();
    let mut body: Vec<String> = Vec::new();
    for (label, result) in [("repo", &repo), ("global", &global)] {
        match result {
            Ok(scoped) => {
                summary.stale = summary.stale.saturating_add(scoped.stale);
                summary.unparsed = summary.unparsed.saturating_add(scoped.unparsed);
                summary.not_loaded = summary.not_loaded.saturating_add(scoped.not_loaded);
                if label == "global" && !scoped.lines.is_empty() {
                    body.push("global:".to_owned());
                }
                body.extend(scoped.lines.iter().cloned());
                if scoped.not_loaded > 0 {
                    let shown = scoped.lines.len();
                    let (cap, limit) = if shown >= LOAD_LINES {
                        ("LOAD_LINES", LOAD_LINES)
                    } else {
                        ("LOAD_BYTES", LOAD_BYTES)
                    };
                    let all = shown.saturating_add(scoped.not_loaded);
                    let next = "await memory.search(\"words\")";
                    let row = crate::listed::cut_row(shown, all, "notes loaded", cap, limit, next);
                    body.push(row);
                }
            }
            Err(error) => body.push(format!("{label} notes not loaded: {error}")),
        }
    }
    summary.repo = repo.as_ref().map_or(0, |scoped| scoped.count);
    summary.global = global.as_ref().map_or(0, |scoped| scoped.count);
    if body.is_empty() {
        body.push("No notes yet.".to_owned());
    }
    let text = format!(
        "{HEADER}\n{} repo · {} global\n{}",
        summary.repo,
        summary.global,
        body.join("\n")
    );
    (text, summary)
}

pub struct MemoryExt {
    home: PathBuf,
    activity: Arc<Activity>,
}

impl MemoryExt {
    pub fn new(home: PathBuf, activity: Arc<Activity>) -> Self {
        Self { home, activity }
    }
}

impl Extension for MemoryExt {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        let Event::SessionStart { cwd, reason } = event else {
            return;
        };
        let sources = Sources::discover(&self.home, cwd);
        for report in claude::run_if_enabled(&self.home, cwd) {
            self.activity.push(report.line());
        }
        let (text, summary) = block_with(&self.home, cwd, &sources);
        if *reason == StartReason::Fresh {
            let counted = Store::new(repo_dir(&self.home, cwd))
                .record(|usage| usage.sessions = usage.sessions.saturating_add(1));
            if let Err(error) = counted {
                self.activity
                    .push(format!("memory · this session was not counted: {error}"));
            }
        }
        self.activity.push(summary.line());
        let budget = yi_context::SourceBudgets::default().memory;
        let text = yi_context::fit(&text, budget, "await memory.search(\"words\")").text;
        out.push(Effect::AttachExternal {
            source: SOURCE.to_owned(),
            trust: Trust::Untrusted,
            text,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::fs;

    fn temp(label: &str) -> Scratch {
        Scratch::new(&format!("yi-memext-{label}")).unwrap()
    }

    #[test]
    fn zero_notes_still_explain_themselves() {
        let home = temp("zero-home");
        let cwd = temp("zero-cwd");
        let (text, summary) = block(&home, &cwd);
        assert!(text.starts_with("Saved memories: notes from earlier sessions, untrusted."));
        assert!(text.ends_with("0 repo · 0 global\nNo notes yet."), "{text}");
        assert!(text.contains(&format!(
            "description: {}\n",
            super::super::doc::PLACEHOLDER
        )));
        assert_eq!(summary.line(), "memory · 0 repo · 0 global");
    }

    #[test]
    fn a_session_start_imports_the_claude_memories_and_announces_them() {
        let home = temp("start-home").join("home");
        let cwd = temp("start-cwd");
        let dir = crate::memory::claude::project_dir(&home, &cwd);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("alpha.md"),
            "---\nname: alpha\ndescription: imported at start\nmetadata:\n  type: project\n---\n\nbody\n",
        )
        .unwrap();
        let activity = Arc::new(Activity::default());
        let mut ext = MemoryExt::new(home.clone(), Arc::clone(&activity));
        let mut out = Vec::new();
        let event = Event::SessionStart {
            cwd: cwd.to_path_buf(),
            reason: StartReason::Fresh,
        };
        ext.on(&event, &mut out);
        let lines = activity.take_lines();
        assert!(
            lines.iter().any(
                |line| line.starts_with("claude · 1 notes in ~/.claude/projects/")
                    && line.ends_with("· 1 imported")
            ),
            "{lines:?}"
        );
        assert_eq!(
            lines.last().map(String::as_str),
            Some("memory · 1 repo · 0 global")
        );
        assert!(
            matches!(out.first(), Some(Effect::AttachExternal { .. })),
            "no start block: {out:?}"
        );
        let Some(Effect::AttachExternal { text, trust, .. }) = out.first() else {
            return;
        };
        assert_eq!(*trust, Trust::Untrusted);
        assert!(
            text.contains("- [alpha](alpha.md) — imported at start"),
            "{text}"
        );
    }

    #[test]
    fn the_claude_fixture_imports_and_renders_the_golden_block() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory");
        let home = temp("golden-home");
        let cwd = temp("golden-cwd");
        let store = Store::new(repo_dir(&home, &cwd));
        let report = store.import(&fixtures.join("claude"), None).unwrap();
        assert_eq!((report.imported, report.updated, report.skipped), (3, 0, 0));
        let (text, summary) = block(&home, &cwd);
        let golden = fs::read_to_string(fixtures.join("claude.v3.block")).unwrap();
        assert_eq!(text, golden.trim_end());
        assert_eq!(summary.unparsed, 1);
        let again = store.import(&fixtures.join("claude"), None).unwrap();
        assert_eq!((again.imported, again.updated, again.skipped), (0, 0, 3));
    }

    #[test]
    fn a_gone_repo_path_is_marked_and_a_host_path_is_not() {
        let root = temp("stale");
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(root.join("scripts/forge_pr.py"), "").unwrap();
        assert_eq!(stale_path("run `scripts/forge_pr.py` first", &root), None);
        assert_eq!(
            stale_path("see scripts/forge.py.", &root).as_deref(),
            Some("scripts/forge.py")
        );
        assert_eq!(
            stale_path("Buildhost /tmp is RAM; ~/.yi/x.json too", &root),
            None
        );
    }
}
