//! What Yi keeps under its home and when an unused entry goes. Each row judges an entry by
//! liveness first and age second; an entry whose liveness cannot be read is kept.
use std::path::{Path, PathBuf};
use std::time::Duration;

const ORPHAN_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
const SNAPSHOTS_KEPT: Duration = Duration::from_secs(30 * 24 * 3600);
const SWEEP_EVERY: Duration = Duration::from_secs(3600);
const DONE: &str = "done";
const STAMP: &str = ".yi/retention.stamp";
const LOCK: &str = ".yi/retention.flock";
const TOMB: &str = "gone";
/// A project whose nearest surviving ancestor is one of these sat on a volume now unmounted.
const MOUNT_ROOTS: [&str; 4] = ["/Volumes", "/mnt", "/media", "/run/media"];
const USER_MOUNTS: [&str; 2] = ["/media", "/run/media"];

enum Fate {
    Keep,
    Remove,
    Trim,
}

struct Row {
    store: &'static str,
    root: fn(&Path) -> PathBuf,
    judge: fn(&Path, Duration) -> Fate,
    trim: fn(&Path) -> bool,
}

const ROWS: [Row; 1] = [Row {
    store: "checkpoints",
    root: crate::checkpoint::checkpoint_root,
    judge: checkpoint_fate,
    trim: trim_checkpoint,
}];

/// Invariant: every capture rewrites the shadow index through `index.lock`, so the shadow
/// dir's mtime is its last capture. A live repo is trimmed once idle an hour, never removed.
fn checkpoint_fate(dir: &Path, idle: Duration) -> Fate {
    let Some(project) = yi_tools::checkpoint::work_tree(dir) else {
        return Fate::Keep;
    };
    let unmounted = project
        .ancestors()
        .find(|up| up.exists())
        .is_some_and(|up| {
            MOUNT_ROOTS.iter().any(|root| up == Path::new(root))
                || up
                    .parent()
                    .is_some_and(|parent| USER_MOUNTS.iter().any(|root| parent == Path::new(root)))
        });
    match project.try_exists() {
        Ok(false) if !unmounted && idle > ORPHAN_AFTER => Fate::Remove,
        Ok(false) | Err(_) => Fate::Keep,
        Ok(true) if idle > SWEEP_EVERY => Fate::Trim,
        Ok(true) => Fate::Keep,
    }
}

fn trim_checkpoint(dir: &Path) -> bool {
    yi_tools::checkpoint::prune(dir, SNAPSHOTS_KEPT).is_ok()
}

/// One store's sweep, printed by `yi gc`: kept over found is what survived.
#[derive(Default)]
pub struct Report {
    store: &'static str,
    found: usize,
    removed: usize,
    trimmed: usize,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            store,
            found,
            removed,
            trimmed,
        } = self;
        let kept = found.saturating_sub(*removed);
        write!(
            f,
            "{store}: kept {kept} of {found}, removed {removed}, trimmed {trimmed}"
        )
    }
}

pub fn sweep(home: &Path, dry_run: bool) -> Vec<Report> {
    ROWS.iter()
        .map(|row| sweep_row(row, home, dry_run))
        .collect()
}

fn sweep_row(row: &Row, home: &Path, dry_run: bool) -> Report {
    let mut report = Report {
        store: row.store,
        ..Report::default()
    };
    for entry in std::fs::read_dir((row.root)(home))
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        if entry.path().extension().is_some_and(|ext| ext == TOMB) {
            let _retried_next_sweep = dry_run || std::fs::remove_dir_all(entry.path()).is_ok();
            continue;
        }
        report.found = report.found.saturating_add(1);
        let idle = meta.modified().ok().and_then(|at| at.elapsed().ok());
        match (row.judge)(&entry.path(), idle.unwrap_or_default()) {
            Fate::Keep => {}
            Fate::Trim => {
                if dry_run || (row.trim)(&entry.path()) {
                    report.trimmed = report.trimmed.saturating_add(1);
                }
            }
            Fate::Remove => {
                // Incident: a delete cut short left a repo a capture could not use; a rename is whole.
                let tomb = entry.path().with_extension(TOMB);
                if dry_run {
                    report.removed = report.removed.saturating_add(1);
                } else if std::fs::rename(entry.path(), &tomb).is_ok() {
                    report.removed = report.removed.saturating_add(1);
                    let _finished_by_the_next_sweep = std::fs::remove_dir_all(tomb);
                }
            }
        }
    }
    report
}

/// About once an hour per home, on a thread the caller never joins; `None` when skipped. The
/// stamp is written once a sweep ends and the flock dies with its holder, so a killed sweep holds nothing.
pub fn sweep_home_hourly(home: &Path) -> Option<std::thread::JoinHandle<()>> {
    let stamp = home.join(STAMP);
    let fresh = std::fs::metadata(&stamp)
        .and_then(|meta| meta.modified())
        .is_ok_and(|at| at.elapsed().is_ok_and(|age| age < SWEEP_EVERY));
    if fresh && std::fs::read_to_string(&stamp).is_ok_and(|mark| mark == DONE) {
        return None;
    }
    let lock = yi_session::try_lock_file(&home.join(LOCK)).ok()?.ok()?;
    let home = home.to_path_buf();
    Some(std::thread::spawn(move || {
        drop(sweep(&home, false));
        let _ = std::fs::write(stamp, DONE);
        drop(lock);
    }))
}
