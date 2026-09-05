//! The lane controls, each red when its control is deleted: a claim never touches the
//! trunk, a full pool refuses, a resumed session gets its slot back, an orphan reaps
//! without losing its branch, and the warmer refuses a lockfile no session synced.

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_runtime::lane::land::{bounded_name, format_lanes, owner_repo, pr_number};
use yi_runtime::lane::{BranchName, ClaimBase, LaneError, Pool, SlotIndex, SlotView, toolchain};
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
        matches!(&held[..], [SlotView::Held { session, .. }] if session == "s-one"),
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
        matches!(&pool.list()?[..], [SlotView::Orphan { session, .. }] if session == "s-crash")
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
    let text = format_lanes(&pool.list()?);
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
