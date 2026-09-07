//! The lane controls, each red when its control is deleted: a claim never touches the
//! trunk, a full pool refuses, a resumed session gets its slot back, an orphan reaps
//! without losing its branch, and the warmer refuses a lockfile no session synced.

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_runtime::lane::land::{
    LaneHandle, bounded_name, format_lanes, open_pr_in, owner_repo, parse_jobs, pr_number,
};
use yi_runtime::lane::{
    BranchName, ClaimBase, Head, LaneError, Pool, SlotIndex, SlotView, TreeState, head, toolchain,
};
use yi_types::lane::PrNumber;

type TestResult = Result<(), Box<dyn Error>>;

fn git(cwd: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = yi_tools::command("git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

struct Rig {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Rig {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("yi-lanes-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&home)?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        git(&repo, &["config", "user.email", "lanes@test"])?;
        git(&repo, &["config", "user.name", "lanes"])?;
        std::fs::write(repo.join("README.md"), "base\n")?;
        git(&repo, &["add", "README.md"])?;
        git(&repo, &["commit", "-qm", "base"])?;
        Ok(Self { root, repo, home })
    }

    fn pool(&self, slots: u8) -> Result<Pool, Box<dyn Error>> {
        Ok(Pool::open(&self.home, &self.repo, slots)?)
    }

    fn reclaim(self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_claim_branches_a_slot_and_leaves_the_trunk_untouched() -> TestResult {
    let rig = Rig::new("claim")?;
    let pool = rig.pool(2)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    assert!(
        !lane.path().starts_with(&rig.repo),
        "the slot lives outside the checkout: {}",
        lane.path().display()
    );
    assert_eq!(
        git(lane.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/s-one"
    );
    assert!(
        lane.path().join("README.md").is_file(),
        "the slot is a real checkout"
    );
    assert_eq!(
        git(&rig.repo, &["symbolic-ref", "--short", "HEAD"])?,
        "main"
    );
    assert_eq!(git(&rig.repo, &["status", "--porcelain"])?, "");
    assert_eq!(
        lane.base().as_deref(),
        Some(git(&rig.repo, &["rev-parse", "HEAD"])?.as_str())
    );
    let held = pool.list()?;
    assert!(
        matches!(&held[..], [SlotView::Held { holder, .. }] if holder.session == "s-one"),
        "{held:?}"
    );
    lane.release()?;
    assert!(
        matches!(&pool.list()?[..], [SlotView::Idle { .. }]),
        "released slot reads idle"
    );
    assert_eq!(
        git(&rig.repo, &["branch", "--list", "yi/s-one"])?,
        "",
        "a branch with nothing on it is deleted at release"
    );
    rig.reclaim();
    Ok(())
}

#[test]
fn a_full_pool_refuses_with_the_count_it_holds() -> TestResult {
    let rig = Rig::new("full")?;
    let pool = rig.pool(2)?;
    let first = pool.claim("s-a", ClaimBase::Main)?;
    let second = pool.claim("s-b", ClaimBase::Main)?;
    assert_ne!(first.slot(), second.slot());
    let refused = pool.claim("s-c", ClaimBase::Main);
    assert!(
        matches!(
            refused,
            Err(LaneError::PoolFull {
                slots: 2,
                held: 2,
                orphans: 0
            })
        ),
        "{refused:?}"
    );
    drop(first);
    drop(second);
    rig.reclaim();
    Ok(())
}

/// A crash leaves the slot on its branch and the state file naming the session under
/// a free lock; that is the orphan shape, written here the way the crash would leave it.
fn orphan(pool: &Pool, slot: SlotIndex, session: &str) -> TestResult {
    git(
        &pool.dir().join(slot.to_string()),
        &["checkout", "-q", &format!("yi/{session}")],
    )?;
    let path = pool.dir().join(format!("{slot}.json"));
    let mut state: yi_types::lane::SlotState = serde_json::from_slice(&std::fs::read(&path)?)?;
    state.session = Some(session.to_owned());
    std::fs::write(&path, serde_json::to_vec(&state)?)?;
    Ok(())
}

#[test]
fn a_resumed_session_reclaims_its_slot_with_its_work_and_an_orphan_blocks_others() -> TestResult {
    let rig = Rig::new("resume")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-crash", ClaimBase::Main)?;
    let slot = lane.slot();
    std::fs::write(lane.path().join("work.txt"), "kept\n")?;
    git(lane.path(), &["add", "work.txt"])?;
    git(lane.path(), &["commit", "-qm", "work"])?;
    drop(lane);
    // The branch survived the drop because main does not contain it.
    assert_eq!(
        git(&rig.repo, &["branch", "--list", "yi/s-crash"])?,
        "yi/s-crash"
    );
    orphan(&pool, slot, "s-crash")?;
    assert!(
        matches!(&pool.list()?[..], [SlotView::Orphan { holder, .. }] if holder.session == "s-crash")
    );
    let refused = pool.claim("s-other", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { orphans: 1, .. })),
        "{refused:?}"
    );
    let back = pool.claim("s-crash", ClaimBase::Main)?;
    assert_eq!(back.slot(), slot);
    assert_eq!(
        git(back.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/s-crash"
    );
    assert!(
        back.path().join("work.txt").is_file(),
        "the commit is still on the branch"
    );
    drop(back);
    rig.reclaim();
    Ok(())
}

/// A `pid-` session cannot be resumed and a drive that died at startup left three of them:
/// the pool was full of slots holding nothing, and only a hand-run reap freed it.
#[test]
fn an_orphan_with_nothing_main_lacks_is_a_free_slot() -> TestResult {
    let rig = Rig::new("abandoned")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("pid-1", ClaimBase::Main)?;
    let slot = lane.slot();
    drop(lane);
    // A clean drop deletes the merged branch; a crash leaves it, so put it back as the crash would.
    git(&rig.repo, &["branch", "-q", "yi/pid-1", "main"])?;
    orphan(&pool, slot, "pid-1")?;
    let taken = pool.claim("s-next", ClaimBase::Main)?;
    assert_eq!(taken.slot(), slot);
    assert_eq!(
        git(taken.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/s-next"
    );
    assert_eq!(
        git(&rig.repo, &["branch", "--list", "yi/pid-1"])?,
        "",
        "nothing to keep"
    );
    drop(taken);
    // Uncommitted work is work: the same shape with a stray file still blocks.
    let lane = pool.claim("pid-2", ClaimBase::Main)?;
    std::fs::write(lane.path().join("draft.txt"), "unsaved\n")?;
    drop(lane);
    git(&rig.repo, &["branch", "-q", "yi/pid-2", "main"])?;
    orphan(&pool, slot, "pid-2")?;
    let refused = pool.claim("s-late", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { orphans: 1, .. })),
        "{refused:?}"
    );
    rig.reclaim();
    Ok(())
}

#[test]
fn binding_the_store_id_renames_the_branch_so_a_resume_finds_it() -> TestResult {
    let rig = Rig::new("bind")?;
    let pool = rig.pool(1)?;
    let mut lane = pool.claim("pid-7", ClaimBase::Main)?;
    lane.bind_session("real-id")?;
    assert_eq!(
        git(lane.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/real-id"
    );
    std::fs::write(lane.path().join("w.txt"), "w\n")?;
    git(lane.path(), &["add", "w.txt"])?;
    git(lane.path(), &["commit", "-qm", "w"])?;
    let slot = lane.slot();
    drop(lane);
    orphan(&pool, slot, "real-id")?;
    let back = pool.claim("real-id", ClaimBase::Main)?;
    assert!(
        back.path().join("w.txt").is_file(),
        "the resumed session gets its work back"
    );
    drop(back);
    rig.reclaim();
    Ok(())
}

#[test]
fn reap_frees_an_orphan_and_keeps_its_unmerged_branch() -> TestResult {
    let rig = Rig::new("reap")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-left", ClaimBase::Main)?;
    let slot = lane.slot();
    std::fs::write(lane.path().join("left.txt"), "unmerged\n")?;
    git(lane.path(), &["add", "left.txt"])?;
    git(lane.path(), &["commit", "-qm", "left"])?;
    drop(lane);
    orphan(&pool, slot, "s-left")?;
    let message = pool.reap(slot)?;
    assert!(message.contains("kept"), "{message}");
    assert_eq!(
        git(&rig.repo, &["branch", "--list", "yi/s-left"])?,
        "yi/s-left"
    );
    assert!(matches!(&pool.list()?[..], [SlotView::Idle { .. }]));
    let text = format_lanes(&pool.list()?, None);
    assert!(text.starts_with("lane 0: idle"), "{text}");
    rig.reclaim();
    Ok(())
}

#[test]
fn a_child_lane_branches_from_the_parents_head() -> TestResult {
    let rig = Rig::new("child")?;
    let pool = rig.pool(2)?;
    let parent = pool.claim("s-parent", ClaimBase::Main)?;
    std::fs::write(parent.path().join("parent.txt"), "parent\n")?;
    git(parent.path(), &["add", "parent.txt"])?;
    git(parent.path(), &["commit", "-qm", "parent work"])?;
    let head = git(parent.path(), &["rev-parse", "HEAD"])?;
    let child = pool.claim("sub-1", ClaimBase::Commit(head.clone()))?;
    assert!(
        child.path().join("parent.txt").is_file(),
        "the child sees the parent's commit"
    );
    std::fs::write(child.path().join("child.txt"), "child\n")?;
    child.merge_into(parent.path(), "sub-1")?;
    assert!(
        parent.path().join("child.txt").is_file(),
        "the merge landed in the parent's lane"
    );
    assert!(
        !rig.repo.join("child.txt").exists(),
        "the trunk checkout is never the merge target"
    );
    drop(parent);
    rig.reclaim();
    Ok(())
}

#[test]
fn branch_names_reject_flags_and_traversal() {
    for bad in ["", "-x", "a..b", "a b", "a//b", "a/", "a.lock"] {
        assert!(BranchName::parse(bad).is_err(), "{bad:?} must be refused");
    }
    assert_eq!(
        BranchName::for_session("pid-42")
            .map(|name| name.as_str().to_owned())
            .ok(),
        Some("yi/pid-42".to_owned())
    );
}

#[test]
fn forge_replies_parse_to_typed_numbers_and_slugs() {
    assert_eq!(
        pr_number("Created https://git.example.invalid/apex/yi/pulls/191"),
        Some(PrNumber(191))
    );
    assert_eq!(pr_number("#12 Title\nhttps://x/y"), Some(PrNumber(12)));
    assert_eq!(pr_number("nothing here"), None);
    assert_eq!(
        owner_repo("ssh://git@forge.example.invalid:2222/apex/yi.git").as_deref(),
        Some("apex/yi")
    );
    assert_eq!(
        owner_repo("git@github.com:owner/repo.git").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(owner_repo("https://github.com/o/r").as_deref(), Some("o/r"));
    let long = "j".repeat(4096);
    assert_eq!(bounded_name(&long).chars().count(), 64);
}

#[test]
fn the_warmer_refuses_a_lockfile_no_session_synced() -> TestResult {
    let rig = Rig::new("warm")?;
    std::fs::write(rig.repo.join("Cargo.lock"), "# lock\n")?;
    git(&rig.repo, &["add", "Cargo.lock"])?;
    git(&rig.repo, &["commit", "-qm", "lock"])?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-warm", ClaimBase::Main)?;
    let slot = lane.slot();
    assert!(
        matches!(
            toolchain::warm(&pool, slot),
            Err(LaneError::LockfileUnseen { .. })
        ),
        "an unseen hash stays cold"
    );
    assert_eq!(lane.sync()?.as_deref(), Some("Cargo.lock"));
    assert_eq!(
        lane.sync()?,
        None,
        "an unchanged hash makes the next claim git-only"
    );
    lane.release()?;
    let state: yi_types::lane::SlotState =
        serde_json::from_slice(&std::fs::read(pool.dir().join(format!("{slot}.json")))?)?;
    assert!(
        state.warm.is_some(),
        "the release started a warm with a receipt"
    );
    rig.reclaim();
    Ok(())
}

/// The one HEAD reader: a branch, a detached sha, and a refusal for anything else.
#[test]
fn head_reads_a_branch_a_detached_sha_and_refuses_garbage() -> TestResult {
    let rig = Rig::new("head")?;
    assert!(
        matches!(head(&rig.repo)?, Head::Branch(branch) if branch.as_str() == "main"),
        "on main"
    );
    git(&rig.repo, &["checkout", "-q", "--detach"])?;
    let sha = git(&rig.repo, &["rev-parse", "HEAD"])?;
    let Head::Detached(detached) = head(&rig.repo)? else {
        return Err("detached HEAD read as a branch".into());
    };
    assert_eq!(detached.as_str(), sha);
    assert_eq!(detached.short(), &sha[..8]);
    std::fs::write(rig.repo.join(".git/HEAD"), "ref: refs/heads/-flag\n")?;
    assert!(head(&rig.repo).is_err(), "a flag-shaped ref is refused");
    std::fs::write(rig.repo.join(".git/HEAD"), "garbage\n")?;
    assert!(
        head(&rig.repo).is_err(),
        "neither a ref nor a sha is refused"
    );
    rig.reclaim();
    Ok(())
}

/// The listing says what a reap would lose before anyone types it: an untracked path
/// on a merged branch is named, and a clean merged slot is the next claim's.
#[test]
fn a_listing_says_what_a_reap_would_lose() -> TestResult {
    let rig = Rig::new("verdict")?;
    let pool = rig.pool(2)?;
    let dirty = pool.claim("s-dirty", ClaimBase::Main)?;
    std::fs::write(dirty.path().join("scratch.txt"), "only copy\n")?;
    let dirty_slot = dirty.slot();
    drop(dirty);
    // A merged branch goes at release; the crash left it, so it is put back as the crash would.
    git(
        &pool.dir().join(dirty_slot.to_string()),
        &["branch", "-q", "yi/s-dirty", "HEAD"],
    )?;
    orphan(&pool, dirty_slot, "s-dirty")?;
    let clean = pool.claim("s-clean", ClaimBase::Main)?;
    let clean_slot = clean.slot();
    drop(clean);
    git(
        &pool.dir().join(clean_slot.to_string()),
        &["branch", "-q", "yi/s-clean", "HEAD"],
    )?;
    orphan(&pool, clean_slot, "s-clean")?;
    let views = pool.list()?;
    let [
        SlotView::Orphan { holder: first, .. },
        SlotView::Orphan { holder: second, .. },
    ] = &views[..]
    else {
        return Err(format!("two orphans: {views:?}").into());
    };
    assert_eq!(
        first.tree,
        TreeState::Known {
            modified: 0,
            untracked: 1,
            ahead: 0,
            behind: 0
        }
    );
    assert!(second.tree.is_empty(), "{:?}", second.tree);
    assert!(first.age.is_some(), "the branch's reflog gives its age");
    let text = format_lanes(&views, None);
    let mut lines = text.lines();
    let (Some(one), Some(two)) = (lines.next(), lines.next()) else {
        return Err(text.into());
    };
    assert!(
        one.contains("1 untracked") && one.contains("uncommitted paths are lost"),
        "{one}"
    );
    assert!(
        one.contains(&pool.dir().to_string_lossy().to_string()),
        "the path is on the line: {one}"
    );
    assert!(
        two.contains("clean") && two.ends_with("the next claim takes it"),
        "{two}"
    );
    rig.reclaim();
    Ok(())
}

/// The ledger names a holder the way the person named the session, and says which root.
#[test]
fn the_ledger_names_a_holder() -> TestResult {
    let rig = Rig::new("ledger")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("01a0-session", ClaimBase::Main)?;
    let mut ledger = yi_types::acp::DaemonLedger {
        sessions: Default::default(),
    };
    ledger.sessions.insert(
        "01a0-session".to_owned(),
        yi_types::acp::DaemonLedgerEntry {
            cwd: "/work/yi".to_owned(),
            unseen: 0,
            last_state: Some("idle".to_owned()),
            last_event_ms: 0,
            name: Some("fix the auth\x1b[31m bug".to_owned()),
            extra: Default::default(),
        },
    );
    let text = format_lanes(&pool.list()?, Some(&ledger));
    assert!(
        text.contains("held by 01a0-session (fix the auth[31m bug, /work/yi) on yi/01a0-session"),
        "{text}"
    );
    lane.release()?;
    rig.reclaim();
    Ok(())
}

/// The prompt's answer lands only on the slot it was asked about.
#[test]
fn reap_left_by_refuses_a_slot_that_moved() -> TestResult {
    let rig = Rig::new("moved")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-first", ClaimBase::Main)?;
    let slot = lane.slot();
    drop(lane);
    git(
        &pool.dir().join(slot.to_string()),
        &["branch", "-q", "yi/s-second", "HEAD"],
    )?;
    orphan(&pool, slot, "s-second")?;
    let refused = pool.reap_left_by(slot, "s-first");
    assert!(
        matches!(refused, Err(LaneError::SlotChanged { .. })),
        "{refused:?}"
    );
    assert!(pool.reap_left_by(slot, "s-second").is_ok());
    rig.reclaim();
    Ok(())
}

/// A rollup with more checks than the row can carry is cut at the parser, not the row.
#[test]
fn a_rollup_is_bounded_at_the_parser() {
    let entries: Vec<serde_json::Value> = (0..300)
        .map(|n| serde_json::json!({"name": format!("check-{n}"), "status": "success"}))
        .collect();
    let jobs = parse_jobs(&entries);
    assert_eq!(jobs.len(), 33);
    assert_eq!(jobs[32].name, "+268 more");
}

type Steers = std::sync::mpsc::Receiver<String>;

fn handle_for(
    lane: yi_runtime::lane::Lane,
    land_command: Option<Vec<String>>,
) -> (std::sync::Arc<LaneHandle>, Steers) {
    let (events, _keep) = tokio::sync::broadcast::channel(16);
    let (tx, rx) = std::sync::mpsc::channel();
    let steer: std::sync::Arc<yi_runtime::lane::land::SteerFn> =
        std::sync::Arc::new(move |message, _| {
            let _ = tx.send(format!("{message:?}"));
        });
    (
        LaneHandle::new(Some(lane), None, land_command, events, steer),
        rx,
    )
}

/// Two `/land` calls are one poller; the second is refused while the first runs.
#[test]
fn two_lands_share_one_poller() -> TestResult {
    let rig = Rig::new("poller")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-poll", ClaimBase::Main)?;
    let slow = vec!["sh".to_owned(), "-c".to_owned(), "sleep 1".to_owned()];
    let (handle, _steers) = handle_for(lane, Some(slow));
    handle.land("Title")?;
    let refused = handle.land("Title");
    assert!(
        matches!(&refused, Err(error) if error.to_string().contains("landing in progress")),
        "{refused:?}"
    );
    std::thread::sleep(std::time::Duration::from_millis(1_800));
    assert!(
        handle.land("Title").is_ok(),
        "the poller is free once the first ends"
    );
    std::thread::sleep(std::time::Duration::from_millis(1_500));
    handle.release()?;
    rig.reclaim();
    Ok(())
}

/// A base that conflicts stops the landing before the push: the steer names the file,
/// the tree is clean again, and the forge never sees the branch.
#[test]
fn a_conflicting_base_pushes_nothing() -> TestResult {
    let rig = Rig::new("conflict")?;
    let bare = rig.root.join("origin.git");
    std::fs::create_dir_all(&bare)?;
    git(&bare, &["init", "-q", "--bare", "-b", "main"])?;
    git(
        &rig.repo,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    )?;
    git(&rig.repo, &["push", "-q", "-u", "origin", "main"])?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-land", ClaimBase::Main)?;
    std::fs::write(lane.path().join("README.md"), "lane\n")?;
    git(lane.path(), &["commit", "-qam", "lane edit"])?;
    std::fs::write(rig.repo.join("README.md"), "trunk\n")?;
    git(&rig.repo, &["commit", "-qam", "trunk edit"])?;
    git(&rig.repo, &["push", "-q", "origin", "main"])?;
    let lane_path = lane.path().to_path_buf();
    let (handle, steers) = handle_for(lane, None);
    handle.land("Title")?;
    let steer = steers.recv_timeout(std::time::Duration::from_secs(60))?;
    assert!(
        steer.contains("conflicts") && steer.contains("README.md"),
        "{steer}"
    );
    assert_eq!(
        git(&lane_path, &["status", "--porcelain"])?,
        "",
        "the merge was aborted"
    );
    assert_eq!(
        git(&bare, &["branch", "--list", "yi/*"])?,
        "",
        "nothing was pushed"
    );
    assert_eq!(handle.landing(), yi_types::lane::Landing::Unlanded);
    handle.release()?;
    rig.reclaim();
    Ok(())
}

/// The forge's open list is read back to a typed number by head branch, on either forge.
#[test]
fn an_open_pull_request_is_found_by_head() {
    let tsv = "12\tyi/other\n191\tyi/01a0\n";
    assert_eq!(open_pr_in(tsv, "yi/01a0"), Some(PrNumber(191)));
    assert_eq!(open_pr_in(tsv, "yi/none"), None);
    assert_eq!(
        open_pr_in(r#"[{"number": 7}]"#, "yi/01a0"),
        Some(PrNumber(7))
    );
    assert_eq!(open_pr_in("[]", "yi/01a0"), None);
    assert_eq!(open_pr_in("garbage", "yi/01a0"), None);
}
