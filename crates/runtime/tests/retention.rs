//! The retention sweep over shadow checkpoint repos written by the real checkpointer.
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use yi_tools::checkpoint::Checkpoints;

use crate::scratch::Scratch;

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

#[expect(
    clippy::disallowed_methods,
    reason = "the sweep reads real ages, so the fixture sets real ones"
)]
fn ago(age: Duration) -> SystemTime {
    SystemTime::now() - age
}

fn age_dir(dir: &Path, age: Duration) -> std::io::Result<()> {
    std::fs::File::open(dir)?.set_modified(ago(age))
}

/// Ages every object written in the last `within`, so captures can sit a day or forty apart.
fn age_objects(git_dir: &Path, within: Duration, age: Duration) -> std::io::Result<()> {
    let mut stack = vec![git_dir.join("objects")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)?.flatten() {
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else if entry.metadata()?.modified()? > ago(within) {
                std::fs::File::open(entry.path())?.set_modified(ago(age))?;
            }
        }
    }
    Ok(())
}

/// One captured project; the shadow dir is the only entry the checkpointer adds to `root`.
fn captured(
    scratch: &Scratch,
    root: &Path,
    name: &str,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let project = scratch.join(name);
    std::fs::create_dir_all(&project)?;
    std::fs::write(project.join("main.rs"), "fn main() {}\n")?;
    let before: Vec<_> = std::fs::read_dir(root)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    Checkpoints::open(root, &project)?.capture()?;
    let shadow = std::fs::read_dir(root)?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| !before.contains(path))
        .ok_or("the capture made no shadow dir")?;
    Ok((project, shadow))
}

#[test]
fn only_an_orphan_past_seven_days_is_removed() -> Result<(), Box<dyn Error>> {
    let scratch = Scratch::new("yi-retention")?;
    let home = scratch.join("home");
    let root = yi_runtime::checkpoint::checkpoint_root(&home);
    let (gone_old, orphan_old) = captured(&scratch, &root, "gone-old")?;
    let (gone_young, orphan_young) = captured(&scratch, &root, "gone-young")?;
    let (nested, nested_orphan) = captured(&scratch, &root, "removed/project")?;
    let (_mounted, behind_mount) = captured(&scratch, &root, "mounted")?;
    // The real capture's config, its work tree moved onto a volume that is not mounted.
    let away = format!(
        "\tworktree = /Volumes/yi-absent-{}/project",
        std::process::id()
    );
    let config: Vec<String> = std::fs::read_to_string(behind_mount.join("config"))?
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("worktree = ") {
                away.clone()
            } else {
                line.to_owned()
            }
        })
        .collect();
    std::fs::write(behind_mount.join("config"), config.join("\n"))?;
    let (_idle, idle_live) = captured(&scratch, &root, "idle")?;
    // git escapes the backslash in its config, so the path read back is not this one.
    let (_escaped, escaped) = captured(&scratch, &root, "back\\slash")?;
    // A parent turned file makes stat fail (ENOTDIR) for any user, root included.
    let (unreadable, unreadable_shadow) = captured(&scratch, &root, "blocked/project")?;
    let foreign = root.join("not-a-capture");
    std::fs::create_dir_all(&foreign)?;
    let tomb = root.join("deadbeef.gone");
    std::fs::create_dir_all(tomb.join("objects"))?;
    std::fs::remove_dir_all(gone_old)?;
    std::fs::remove_dir_all(gone_young)?;
    std::fs::remove_dir_all(nested.parent().ok_or("no parent")?)?;
    let blocked = unreadable.parent().ok_or("no parent")?;
    std::fs::remove_dir_all(blocked)?;
    std::fs::write(blocked, "")?;
    age_dir(&orphan_old, 7 * DAY + HOUR)?;
    age_dir(&orphan_young, 7 * DAY - HOUR)?;
    for dir in [
        &nested_orphan,
        &behind_mount,
        &unreadable_shadow,
        &idle_live,
        &escaped,
        &foreign,
    ] {
        age_dir(dir, 400 * DAY)?;
    }

    let preview: Vec<String> = yi_runtime::retention::sweep(&home, true)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(orphan_old.is_dir() && tomb.is_dir(), "a preview deleted");
    let swept: Vec<String> = yi_runtime::retention::sweep(&home, false)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(preview, swept, "the preview reported a different sweep");
    assert!(
        swept.contains(&"checkpoints: kept 6 of 8, removed 2, trimmed 1".to_owned()),
        "{swept:?}"
    );
    assert!(!orphan_old.exists(), "an orphan past seven days was kept");
    assert!(
        !nested_orphan.exists(),
        "an orphan whose parent went too was kept"
    );
    assert!(!tomb.exists(), "a delete cut short was never finished");
    for kept in [
        &orphan_young,
        &behind_mount,
        &unreadable_shadow,
        &idle_live,
        &escaped,
        &foreign,
    ] {
        assert!(kept.is_dir(), "{} was swept", kept.display());
    }
    Ok(())
}

#[test]
fn a_trim_keeps_every_capture_inside_the_window() -> Result<(), Box<dyn Error>> {
    let scratch = Scratch::new("yi-retention-trim")?;
    let home = scratch.join("home");
    let root = yi_runtime::checkpoint::checkpoint_root(&home);
    let (project, shadow) = captured(&scratch, &root, "project")?;
    std::fs::write(project.join("gone.rs"), "// removed before the window\n")?;
    // Files older than the shadow index, as a real project's are, take git's cached-tree path.
    for file in ["main.rs", "gone.rs"] {
        std::fs::File::open(project.join(file))?.set_modified(ago(50 * DAY))?;
    }
    let checkpoints = Checkpoints::open(&root, &project)?;
    let old = checkpoints.capture()?;
    std::fs::remove_file(project.join("gone.rs"))?;
    checkpoints.capture()?;
    age_objects(&shadow, HOUR, 40 * DAY)?;
    let unchanged = checkpoints.capture()?;
    age_objects(&shadow, HOUR, DAY)?;
    std::fs::write(project.join("main.rs"), "fn main() { todo() }\n")?;
    // The shadow index roots the newest capture, so only an older one tests the window.
    checkpoints.capture()?;

    yi_runtime::retention::sweep(&home, false);

    assert!(
        checkpoints.restore(&old, None).is_err(),
        "a capture past the window survived"
    );
    checkpoints.restore(&unchanged, None)?;
    assert_eq!(
        std::fs::read_to_string(project.join("main.rs"))?,
        "fn main() {}\n",
        "a capture of an unchanged project, a day old, was pruned"
    );
    Ok(())
}

#[test]
fn a_session_start_sweeps_a_home_with_no_stamp() -> Result<(), Box<dyn Error>> {
    let scratch = Scratch::new("yi-retention-hourly")?;
    let home = scratch.join("home");
    let root = yi_runtime::checkpoint::checkpoint_root(&home);
    let (gone, orphan) = captured(&scratch, &root, "gone")?;
    std::fs::remove_dir_all(gone)?;
    age_dir(&orphan, 8 * DAY)?;
    yi_runtime::retention::sweep_hourly(&home);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while orphan.exists() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        !orphan.exists(),
        "the first session of the hour swept nothing"
    );
    Ok(())
}
