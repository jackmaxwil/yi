//! The working folder's branch and its uncommitted changes, read with the `git` command.

mod host;
mod merge;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::Duration;

use yd_core::command::tools::changes::MERGE;
use yd_core::feed::{CHANGES, FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::FeedId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::{Row, Value};

use crate::fs::{MAX_TEXT, Mirror, POLL, inside, moved, prefix};

pub const FEED: FeedId = CHANGES;

/// Where the `git` binary may be; the first that exists is used, never a shell lookup.
const GIT: &[&str] = &[
    "/usr/bin/git",
    "/opt/homebrew/bin/git",
    "/usr/local/bin/git",
];
/// The longest a `git` run may take before it and its group are killed.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The most output one run may give: the patch of a whole working tree.
const MAX_OUTPUT: usize = 32 << 20;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("no git binary in {GIT:?}")]
    NoGit,
    #[error("git could not be started: {0}")]
    Start(#[source] std::io::Error),
    #[error("git ran past {TIMEOUT:?} and was stopped")]
    Timeout,
    #[error("git wrote more than {MAX_OUTPUT} bytes")]
    TooLong,
    #[error("git could not read this folder ({})", .0.map_or_else(|| "it was stopped".to_owned(), |c| format!("exit {c}")))]
    Failed(Option<i32>),
}

/// Runs `git -C root <args>` with `input` on its stdin, killed with its group past [`TIMEOUT`].
pub(crate) fn git(root: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>, GitError> {
    let bin = GIT
        .iter()
        .find(|bin| Path::new(bin).exists())
        .ok_or(GitError::NoGit)?;
    #[allow(clippy::disallowed_methods, reason = "the git spawn")]
    let mut child = Command::new(bin)
        .arg("-C")
        .arg(root)
        // Incident: a folder's own `core.fsmonitor` ran as soon as the app looked at it.
        .args(["-c", "core.quotePath=false", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(GitError::Start)?;
    let (mut stdin, stdout) = (child.stdin.take(), child.stdout.take());
    let input = input.to_vec();
    let (done, got) = channel();
    thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(&input);
        }
        drop(stdin);
        let mut out = Vec::new();
        let read = stdout.map(|o| o.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut out));
        let _ = done.send(read.map(|r| r.map(|_| out)));
    });
    let out = match got.recv_timeout(TIMEOUT) {
        Ok(Some(Ok(out))) if out.len() <= MAX_OUTPUT => out,
        Ok(Some(Ok(_))) => return Err(stop(&mut child, GitError::TooLong)),
        Ok(_) | Err(RecvTimeoutError::Disconnected) => Vec::new(),
        Err(RecvTimeoutError::Timeout) => return Err(stop(&mut child, GitError::Timeout)),
    };
    let status = child.wait().map_err(GitError::Start)?;
    if status.success() {
        Ok(out)
    } else {
        Err(GitError::Failed(status.code()))
    }
}

/// Kills `child`'s whole group and reaps it.
fn stop(child: &mut std::process::Child, why: GitError) -> GitError {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
    why
}

/// Fills the feed from the folder `SourceCmd::Root` names, again on every poll; until then
/// nothing is read. A revert names its hunk by the lines shown, so a changed one is refused.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let mut root: Option<PathBuf> = None;
    let mut mirror = Mirror::default();
    let mut said = merge::Said::default();
    loop {
        if let Some(root) = &root {
            publish(&out, &mut mirror, root, &said)?;
        }
        match cmds.recv_timeout(POLL) {
            Ok(SourceCmd::Root(folder)) => {
                if moved(&mut root, folder) {
                    mirror.clear(&out)?;
                }
            }
            Ok(SourceCmd::Revert { file, hunk }) => {
                let (Some(root), Some(file)) = (&root, file.to_str()) else {
                    continue;
                };
                let row = mirror.find(&["change:", file].concat());
                if let Err(why) = revert(root, row, Some(&hunk)) {
                    out.send(FeedEvent::Status(FeedStatus::Error(why)))?;
                }
            }
            Ok(SourceCmd::Run(run)) if run.command == MERGE => {
                if let Some(root) = &root {
                    merge::merge(root, &mut said);
                }
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn publish(
    out: &FeedTx,
    mirror: &mut Mirror,
    root: &Path,
    said: &merge::Said,
) -> Result<(), Closed> {
    match snapshot(root) {
        Ok(mut rows) => {
            rows.extend(merge::row(root, rows.first(), said));
            mirror.publish(out, rows)
        }
        Err(why) => out.send(FeedEvent::Status(FeedStatus::Error(why.to_string()))),
    }
}

/// The `repo` row (`branch`, `commit`, `ahead`, `behind`, `project`, `base`, `host`, and `added`
/// and `removed` over every change) and one `change:<path>` row per changed file, read from `root`.
pub fn snapshot(root: &Path) -> Result<Vec<Row>, GitError> {
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
        b"",
    )?;
    let (repo, mut files) = parse_status(&String::from_utf8_lossy(&status));
    let base = base(root);
    // A Yi checkout's edits may be committed on its branch: they are measured from the base.
    let lane = repo
        .words("branch")
        .is_some_and(|branch| branch.starts_with(LANE));
    let fork = base.as_deref().filter(|_| lane).and_then(|base| {
        let out = git(root, &["merge-base", "HEAD", base], b"").ok()?;
        Some(String::from_utf8_lossy(&out).trim().to_owned())
    });
    // Invariant: a repository with no commit yet has no HEAD to diff against.
    let against = fork.as_deref().unwrap_or("HEAD");
    let tracked = git(
        root,
        &[
            "diff",
            against,
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
        ],
        b"",
    )
    .unwrap_or_default();
    let mut patches = split_patch(&String::from_utf8_lossy(&tracked));
    let committed = patches
        .iter()
        .filter(|(path, _)| !files.iter().any(|(p, _)| p == *path));
    let committed: Vec<(String, String)> = committed
        .map(|(path, patch)| (path.clone(), committed_letter(patch).to_owned()))
        .collect();
    files.extend(committed);
    let mut rows = Vec::new();
    let mut total = (0usize, 0usize);
    for (path, letter) in files {
        let patch = match patches.remove(&path) {
            Some(patch) => patch,
            None if letter == "A" => new_file(root, &path),
            None => String::new(),
        };
        let (added, removed) = counts(&patch);
        total = (
            total.0.saturating_add(added),
            total.1.saturating_add(removed),
        );
        rows.push(
            Row::new(["change:", &path].concat())
                .text("path", path)
                .text("status", letter)
                .text("added", added.to_string())
                .text("removed", removed.to_string())
                .text("patch", cut(patch)),
        );
    }
    let project = root.file_name().and_then(|name| name.to_str());
    let mut repo = repo
        .text("project", project.unwrap_or_default())
        .text("added", total.0.to_string())
        .text("removed", total.1.to_string());
    rows.extend(checkouts(root, base.as_deref().unwrap_or("HEAD")));
    if let Some(base) = base {
        repo = repo.text("base", base);
    }
    if let Some(host) = host::name() {
        repo = repo.text("host", host);
    }
    rows.insert(0, repo);
    Ok(rows)
}

/// The prefix of the branch Yi gives a session's own checkout: `yi/<session id>`.
const LANE: &str = "yi/";

/// A file only a commit on the branch changed: new, gone, or changed.
fn committed_letter(patch: &str) -> &'static str {
    if patch.contains("\nnew file mode") {
        "A"
    } else if patch.contains("\ndeleted file mode") {
        "D"
    } else {
        "M"
    }
}

/// One `worktree:<branch>` row with its `path` for each checkout of the repository on a
/// Yi branch, and `changed` when it differs from `base`, so the app can follow a session in.
fn checkouts(root: &Path, base: &str) -> Vec<Row> {
    let Ok(out) = git(root, &["worktree", "list", "--porcelain", "-z"], b"") else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out);
    let mut rows = Vec::new();
    let mut path = None;
    for field in text.split('\0') {
        if let Some(at) = field.strip_prefix("worktree ") {
            path = Some(at.to_owned());
        } else if let Some(branch) = field.strip_prefix("branch refs/heads/")
            && branch.starts_with(LANE)
            && let Some(at) = path.take()
        {
            let at = PathBuf::from(at);
            let edited = git(&at, &["diff", "--quiet", base], b"").is_err();
            let added = git(&at, &["ls-files", "--others", "--exclude-standard"], b"");
            let changed = edited || added.is_ok_and(|listed| !listed.is_empty());
            let row = Row::new(["worktree:", branch].concat()).text("path", at.to_string_lossy());
            rows.push(row.cell("changed", Value::Flag(changed)));
        }
    }
    rows
}

/// The branch the repository is based on: what `origin/HEAD` points at, else `main`, else
/// `master`, whichever exists.
fn base(root: &Path) -> Option<String> {
    let refs = [
        "for-each-ref",
        "--format=%(symref:short) %(refname:short)",
        "refs/remotes/origin/HEAD",
        "refs/heads/main",
        "refs/heads/master",
    ];
    let out = git(root, &refs, b"").ok()?;
    base_of(&String::from_utf8_lossy(&out))
}

/// Reads [`base`]'s listing: one `<what it points at> <its name>` line a ref, local first.
fn base_of(listing: &str) -> Option<String> {
    let refs = || listing.lines().filter_map(|line| line.split_once(' '));
    let remote = refs().find_map(|(points_at, _)| Some(points_at.split_once('/')?.1));
    let local = refs().map(|(_, name)| name).find(|name| !name.is_empty());
    remote.or(local).map(str::to_owned)
}

/// The branch row and each changed path with its status letter.
fn parse_status(text: &str) -> (Row, Vec<(String, String)>) {
    let mut repo = Row::new("repo");
    let mut files = Vec::new();
    let mut fields = text.split('\0');
    while let Some(field) = fields.next() {
        if let Some(head) = field.strip_prefix("# branch.head ") {
            repo = repo.text("branch", head);
        } else if let Some(oid) = field.strip_prefix("# branch.oid ") {
            repo = repo.text("commit", oid.chars().take(7).collect::<String>());
        } else if let Some(ab) = field.strip_prefix("# branch.ab ") {
            let mut parts = ab.split(' ').map(|n| n.trim_start_matches(['+', '-']));
            repo = repo.text("ahead", parts.next().unwrap_or("0"));
            repo = repo.text("behind", parts.next().unwrap_or("0"));
        } else if let Some(path) = field.strip_prefix("? ") {
            files.push((path.to_owned(), "A".to_owned()));
        } else if let Some(rest) = field.strip_prefix(['1', '2', 'u']) {
            let skip = match field.get(..1) {
                Some("1") => 7,
                Some("2") => 8,
                _ => 9,
            };
            let mut parts = rest.trim_start().splitn(skip + 1, ' ');
            let xy = parts.next().unwrap_or("..");
            if let Some(path) = parts.nth(skip - 1) {
                files.push((path.to_owned(), letter(xy).to_owned()));
            }
            if field.starts_with('2') {
                fields.next();
            }
        }
    }
    (repo, files)
}

/// One letter for a status pair: the working tree's change, else the index's.
fn letter(xy: &str) -> &'static str {
    let mut pair = xy.chars();
    let (x, y) = (pair.next().unwrap_or('.'), pair.next().unwrap_or('.'));
    match if y == '.' { x } else { y } {
        'A' | '?' => "A",
        'D' => "D",
        'R' | 'C' => "R",
        'U' => "U",
        _ => "M",
    }
}

/// The whole-tree patch split by file, keyed by the path it changes.
fn split_patch(patch: &str) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    for block in patch
        .split("\ndiff --git ")
        .filter(|b| !b.trim().is_empty())
    {
        let block = block.strip_prefix("diff --git ").unwrap_or(block);
        let named = |mark: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(mark))
                .map(|path| path.trim_end_matches('\t').to_owned())
        };
        if let Some(path) = named("+++ b/").or_else(|| named("--- a/")) {
            files.insert(
                path,
                ["diff --git ", block.trim_end_matches('\n'), "\n"].concat(),
            );
        }
    }
    files
}

/// The patch that adds an untracked file, read from disk.
fn new_file(root: &Path, path: &str) -> String {
    let mut text = Vec::new();
    let read = inside(root, path).map(|at| {
        std::fs::File::open(at).and_then(|f| f.take(MAX_TEXT as u64 + 1).read_to_end(&mut text))
    });
    let Some(Ok(_)) = read else {
        return String::new();
    };
    let head = format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n");
    if text.contains(&0) {
        return [
            head.as_str(),
            &format!("Binary files /dev/null and b/{path} differ\n"),
        ]
        .concat();
    }
    let text = String::from_utf8_lossy(prefix(&text, MAX_TEXT));
    let lines: Vec<&str> = text.lines().collect();
    let mut patch = format!("{head}+++ b/{path}\n@@ -0,0 +1,{} @@\n", lines.len());
    for line in &lines {
        patch.extend(["+", line, "\n"]);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    patch
}

/// Lines added and removed in a patch.
pub fn counts(patch: &str) -> (usize, usize) {
    let body = patch.lines().skip_while(|line| !line.starts_with("@@"));
    body.fold((0, 0), |(add, del), line| match line.as_bytes().first() {
        Some(b'+') => (add.saturating_add(1), del),
        Some(b'-') => (add, del.saturating_add(1)),
        _ => (add, del),
    })
}

/// A patch kept whole up to [`MAX_TEXT`], else cut after its last whole line.
fn cut(patch: String) -> String {
    if patch.len() <= MAX_TEXT {
        return patch;
    }
    let kept = String::from_utf8_lossy(prefix(patch.as_bytes(), MAX_TEXT)).into_owned();
    kept.rsplit_once('\n')
        .map_or_else(String::new, |(head, _)| [head, "\n"].concat())
}

/// The file's header and its `n`th hunk: a patch `git apply` takes alone.
pub fn hunk(patch: &str, n: usize) -> Option<String> {
    let (head, hunks) = patch.split_once("\n@@")?;
    let body = ["@@", hunks].concat();
    let mut pieces = body.split("\n@@").map(|p| p.trim_start_matches("@@"));
    let piece = pieces.nth(n)?;
    Some(
        [
            head,
            "\n@@",
            piece,
            if piece.ends_with('\n') { "" } else { "\n" },
        ]
        .concat(),
    )
}

/// A hunk's lines without its headers, one per line with its sign: what the user confirmed.
pub fn body(hunk: &str) -> String {
    let lines = hunk
        .lines()
        .skip_while(|line| !line.starts_with("@@"))
        .skip(1);
    let lines: Vec<&str> = lines.filter(|line| !line.starts_with('\\')).collect();
    lines.join("\n")
}

/// Puts back the hunk of `row`'s patch whose lines are `shown`; a new file is never deleted.
fn revert(root: &Path, row: Option<&Row>, shown: Option<&str>) -> Result<(), String> {
    let row = row.ok_or("no change for that file")?;
    if row.words("status") == Some("A") {
        return Err("a new file has no earlier version to put back".to_owned());
    }
    let (patch, shown) = (
        row.words("patch").ok_or("no change for that file")?,
        shown.ok_or("a revert carries the hunk's lines")?,
    );
    let one = (0..)
        .map_while(|n| hunk(patch, n))
        .find(|one| body(one) == shown)
        .ok_or("the hunk changed since it was shown; nothing was put back")?;
    git(
        root,
        &["apply", "-R", "--whitespace=nowarn", "-"],
        one.as_bytes(),
    )
    .map(drop)
    .map_err(|why| format!("the hunk was not put back: {why}"))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::fs::tests::{scratch, sh};

    const PATCH: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n-one\n+uno\n two\n@@ -9,1 +9,2 @@\n nine\n+ten\n";

    const TWELVE: [&str; 12] = [
        "one", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12",
    ];

    /// A repository with one commit, one file changed and one file new.
    fn repo(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let at = scratch(name)?;
        sh(&at, &["init", "-q", "-b", "main"])?;
        std::fs::write(at.join("a.rs"), TWELVE.join("\n") + "\n")?;
        sh(&at, &["add", "a.rs"])?;
        sh(
            &at,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "c",
            ],
        )?;
        let changed = ["uno"]
            .iter()
            .chain(TWELVE.get(1..).unwrap_or(&[]))
            .chain(&["thirteen"]);
        std::fs::write(
            at.join("a.rs"),
            changed.copied().collect::<Vec<_>>().join("\n") + "\n",
        )?;
        std::fs::write(at.join("new.rs"), "fn new() {}\n")?;
        Ok(at)
    }

    /// Dies with a folder that runs its own code when the app looks at it: a repository's
    /// `core.fsmonitor` and a textconv driver are commands git starts.
    #[test]
    fn reading_a_folder_never_runs_a_command_its_config_names() -> Result<(), Box<dyn Error>> {
        use std::os::unix::fs::PermissionsExt;
        let at = repo("hostile")?;
        let (hook, ran) = (at.join("hook.sh"), at.join("ran"));
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", ran.display()))?;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        let hook = hook.to_str().ok_or("a path that is not text")?;
        sh(&at, &["config", "core.fsmonitor", hook])?;
        sh(&at, &["config", "diff.evil.textconv", hook])?;
        std::fs::write(at.join(".gitattributes"), "*.rs diff=evil\n")?;
        snapshot(&at)?;
        assert!(!ran.exists(), "git ran the folder's own command");
        Ok(())
    }

    #[test]
    fn a_folder_that_is_not_a_repository_says_so_in_words() -> Result<(), Box<dyn Error>> {
        // Outside the lane: a folder under target/ sits inside this repository.
        let at = std::env::temp_dir().join(format!("yd-not-a-repo-{}", std::process::id()));
        std::fs::create_dir_all(&at)?;
        let said = snapshot(&at).err().map(|e| e.to_string());
        std::fs::remove_dir_all(&at)?;
        assert_eq!(
            said.as_deref(),
            Some("git could not read this folder (exit 128)")
        );
        Ok(())
    }

    fn change<'a>(rows: &'a [Row], path: &str) -> Result<&'a Row, Box<dyn Error>> {
        let id = ["change:", path].concat();
        Ok(rows.iter().find(|r| r.id.as_str() == id).ok_or(id)?)
    }

    #[test]
    fn the_snapshot_names_the_branch_and_each_change_with_counts() -> Result<(), Box<dyn Error>> {
        let rows = snapshot(&repo("snapshot")?)?;
        let repo = rows.first().ok_or("no rows")?;
        assert_eq!(repo.id.as_str(), "repo");
        assert_eq!(repo.words("branch"), Some("main"));
        assert_eq!(repo.words("commit").map(str::len), Some(7));
        let a = change(&rows, "a.rs")?;
        assert_eq!(
            (a.words("status"), a.words("added"), a.words("removed")),
            (Some("M"), Some("2"), Some("1"))
        );
        assert!(a.words("patch").is_some_and(|p| p.contains("+thirteen")));
        let new = change(&rows, "new.rs")?;
        assert_eq!(
            (new.words("status"), new.words("added")),
            (Some("A"), Some("1"))
        );
        Ok(())
    }

    #[test]
    fn the_repo_row_names_its_project_its_base_and_what_changed_in_all()
    -> Result<(), Box<dyn Error>> {
        let at = repo("project")?;
        sh(&at, &["checkout", "-q", "-b", "rate-limit-uploads"])?;
        let rows = snapshot(&at)?;
        let repo = rows.first().ok_or("no rows")?;
        let cell = |name| repo.words(name);
        assert_eq!(repo.id.as_str(), "repo");
        assert_eq!(cell("branch"), Some("rate-limit-uploads"));
        assert_eq!(cell("project"), Some("project"), "the folder's own name");
        assert_eq!(cell("base"), Some("main"));
        assert_eq!((cell("added"), cell("removed")), (Some("3"), Some("1")));
        Ok(())
    }

    /// Dies with Yi's checkout of a second session: its edits are on `yi/<session>`, committed
    /// or not, in a worktree of the project's repository that `git status` there never shows.
    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "needs git 2.36 for `worktree list -z`, and the forge image has 2.34 (#2341)"
    )]
    fn a_sessions_checkout_is_named_and_shows_its_edits_against_the_base()
    -> Result<(), Box<dyn Error>> {
        let at = repo("checkout")?;
        let lane = scratch("checkout-lane")?.join("0");
        let lane_text = lane.to_str().ok_or("a path that is not text")?;
        sh(&at, &["worktree", "add", "-q", "-b", "yi/0f3e", lane_text])?;
        std::fs::write(lane.join("a.rs"), "uno\n")?;
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qam",
            "e",
        ];
        sh(&lane, &commit)?;
        std::fs::write(lane.join("by.rs"), "fn by() {}\n")?;
        let rows = snapshot(&at)?;
        let named = rows.iter().find(|r| r.id.as_str() == "worktree:yi/0f3e");
        let path = named.and_then(|row| row.words("path")).map(PathBuf::from);
        assert_eq!(
            path.map(|p| p.canonicalize()).transpose()?,
            Some(lane.canonicalize()?)
        );
        let changed = named.and_then(|row| row.get("changed"));
        assert_eq!(
            changed,
            Some(&Value::Flag(true)),
            "its edits differ from main"
        );
        let pristine = scratch("checkout-pristine")?.join("0");
        let pristine_text = pristine.to_str().ok_or("a path that is not text")?;
        sh(
            &at,
            &["worktree", "add", "-q", "-b", "yi/77aa", pristine_text],
        )?;
        let rows = snapshot(&at)?;
        let held = rows.iter().find(|r| r.id.as_str() == "worktree:yi/77aa");
        let changed = held.and_then(|row| row.get("changed"));
        assert_eq!(
            changed,
            Some(&Value::Flag(false)),
            "a checkout held with nothing in it"
        );
        let rows = snapshot(&lane)?;
        let a = change(&rows, "a.rs")?;
        assert_eq!(
            (a.words("status"), a.words("removed")),
            (Some("M"), Some("12"))
        );
        assert_eq!(change(&rows, "by.rs")?.words("status"), Some("A"));
        let base = rows.first().and_then(|repo| repo.words("base"));
        assert_eq!(base, Some("main"));
        Ok(())
    }

    #[test]
    fn the_base_is_where_the_remote_points_before_any_local_guess() {
        let cloned = " main\n master\norigin/trunk origin\n";
        assert_eq!(base_of(cloned).as_deref(), Some("trunk"));
        assert_eq!(base_of(" master\n").as_deref(), Some("master"));
        assert_eq!(base_of(""), None);
    }

    #[test]
    fn a_reverted_hunk_puts_those_lines_back_and_keeps_the_rest() -> Result<(), Box<dyn Error>> {
        let at = repo("revert")?;
        let patch = snapshot(&at)?
            .into_iter()
            .find(|r| r.id.as_str() == "change:a.rs");
        let patch = patch
            .and_then(|r| r.words("patch").map(str::to_owned))
            .ok_or("no patch")?;
        let one = hunk(&patch, 0).ok_or("no hunk")?;
        git(&at, &["apply", "-R", "-"], one.as_bytes())?;
        let kept = TWELVE.join("\n") + "\nthirteen\n";
        assert_eq!(
            std::fs::read_to_string(at.join("a.rs"))?,
            kept,
            "only the first hunk goes back"
        );
        assert_eq!(std::fs::read_to_string(at.join("new.rs"))?, "fn new() {}\n");
        Ok(())
    }

    #[test]
    fn a_revert_takes_the_hunk_shown_and_refuses_a_changed_or_new_one() -> Result<(), Box<dyn Error>>
    {
        let at = repo("shown")?;
        let rows = snapshot(&at)?;
        let (a, new) = (change(&rows, "a.rs")?, change(&rows, "new.rs")?);
        let first = hunk(a.words("patch").ok_or("no patch")?, 0).ok_or("no hunk")?;
        let stale = body(&first).replace("+uno", "+eins");
        assert!(revert(&at, Some(a), Some(&stale)).is_err());
        assert!(revert(&at, Some(new), new.words("patch").map(body).as_deref()).is_err());
        assert_eq!(std::fs::read_to_string(at.join("new.rs"))?, "fn new() {}\n");
        let second = hunk(a.words("patch").ok_or("no patch")?, 1).ok_or("no hunk")?;
        revert(&at, Some(a), Some(&body(&second)))?;
        let kept = ["uno"].iter().chain(TWELVE.get(1..).unwrap_or(&[]));
        let kept = kept.copied().collect::<Vec<_>>().join("\n") + "\n";
        assert_eq!(std::fs::read_to_string(at.join("a.rs"))?, kept);
        Ok(())
    }

    #[test]
    fn a_new_file_past_the_text_cap_comes_back_cut() -> Result<(), Box<dyn Error>> {
        let at = scratch("big-new")?;
        let lines = MAX_TEXT / 2;
        std::fs::write(at.join("big.txt"), "x\n".repeat(lines.saturating_add(1000)))?;
        std::fs::write(at.join("edge.txt"), "x\n".repeat(lines))?;
        assert_eq!(counts(&new_file(&at, "big.txt")).0, lines);
        assert_eq!(counts(&new_file(&at, "edge.txt")).0, lines);
        Ok(())
    }

    #[test]
    fn a_hunk_is_cut_with_its_file_header() {
        let second = hunk(PATCH, 1);
        let head = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n";
        assert_eq!(
            second.as_deref(),
            Some([head, "@@ -9,1 +9,2 @@\n nine\n+ten\n"].concat().as_str())
        );
        assert_eq!(hunk(PATCH, 2), None);
        assert_eq!(counts(PATCH), (2, 1));
    }
}
