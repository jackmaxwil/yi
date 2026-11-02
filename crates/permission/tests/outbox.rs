//! What a sweep of the outbox must refuse to take, and the entries no sweep can clear.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use yi_permission::outbox::{DEPTH_MAX, take_refs};

fn fresh(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "yi-outbox-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn a_branch(admin: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(admin.join("refs/heads/main"))?;
    std::fs::write(admin.join("refs/heads/main/keep"), "sha\n")?;
    Ok(())
}

/// A link at the outbox's own path once resolved into `.git` and unlinked a branch of the project.
#[test]
fn a_link_where_the_outbox_should_be_takes_no_ref() -> Result<(), Box<dyn std::error::Error>> {
    let admin = fresh("link");
    a_branch(&admin)?;
    let store = admin.join("yi-outbox");
    std::os::unix::fs::symlink(&admin, &store)?;
    let taken = take_refs(&store, 16);
    assert!(taken.refs.is_empty(), "{:?}", taken.refs);
    assert!(
        admin.join("refs/heads/main/keep").is_file(),
        "a branch was unlinked"
    );
    Ok(())
}

/// A directory past the depth cap is no ref and no sweep clears it, so it is named, never counted
/// as refused: the sweep's own doc says a refused entry is gone now.
#[test]
fn a_directory_past_the_cap_is_named_and_not_counted_as_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let store = fresh("deep");
    let mut deep = store.join("refs");
    for level in 0..=DEPTH_MAX.saturating_add(1) {
        deep = deep.join(format!("d{level}"));
    }
    std::fs::create_dir_all(&deep)?;
    let taken = take_refs(&store, 16);
    assert_eq!(taken.refused, 0, "{taken:?}");
    assert_eq!(taken.too_deep, 1, "{taken:?}");
    Ok(())
}
