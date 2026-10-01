//! The lane controls, each red when its control is deleted: a claim never touches the
//! trunk, a full pool refuses, a resumed session gets its slot back, an orphan reaps
//! without losing its branch, and the warmer refuses a lockfile no session synced.
//!
//! F0d, acceptance. Plan section 6.6 is the invariant the rows below defend: a worktree
//! todo reaches the parent only through a candidate that passed on its own branch and an
//! integration that passed on the merged tree, published only while the parent generation
//! it was prepared against still holds; every other exit records a disposition and never
//! merges to free a slot. The repository these rows run against is built by
//! `fixtures/plans/worktree/repo.sh` through the new `Rig::fixture_repo(Dirt)` helper,
//! which returns the parent checkout and leaves `yi/cand-green`, `yi/cand-red` and
//! `yi/cand-conflict` in it; `fixtures/plans/worktree/README.md` is the rule it obeys.
//! `Rig::new`, `Rig::pool`, `git` and `orphan` keep their present meanings and the
//! existing rows keep using them.
//!
//! | test | tier | helpers | what it pins | the control it dies with |
//! |---|---|---|---|---|
//! | `failing_candidate_never_contaminates_parent_checkout` | T1 | `Rig::fixture_repo(Dirt::Dirty)`, `git` | A candidate the frozen checker refuses leaves the parent's head, index and working tree exactly as they were: no merge, no staging commit, no branch moved. The candidate stays on `yi/cand-red` and is still readable afterwards. | The candidate check running against the candidate commit, before any merge is prepared. Prepare first and check afterwards and the parent already carries the merge when the verdict arrives, so the only way back is a revert nothing records. |
//! | `changed_parent_generation_rejects_stale_integration` | T1 | `Rig::fixture_repo`, `git` | An integration prepared against one generation and published after the parent moved is refused, the prepared tree is dropped, and the retry against the new generation succeeds. The refusal names both generations. | The generation compared at publication and not only at preparation. Compare once and a merge computed against a parent that no longer exists lands as `Done`, which is the lost-update this whole phase exists against. |
//! | `discarded_worktree_is_not_reported_as_merged` | T1 | `Rig::fixture_repo`, `Rig::pool`, `git` | A discarded candidate answers `discarded`, never `merged`, its branch is gone, the pin in `kept` still resolves, and the parent's head did not move. | `take_settled_worktree` taking the lane on discard and on merge alike (`lane/mod.rs:1035-1053`), so the reply is built from the disposition and not from whether `Option<Lane>` was `Some`. Read the reply off the lane and a discard reports a merge that never happened. |
//! | `merge_conflict_retains_recoverable_candidate` | T1 | `Rig::fixture_repo`, `git` | `yi/cand-conflict` against the moved generation records `MergeFailed` with the conflicting path and its merge base, keeps the branch, keeps the slot, and names the conflict the next `submit` resolves. Nothing about the parent moved. | The conflict path writing a disposition rather than returning an error. Return an error and the lane drops through `Drop` with no record, so the branch is the only evidence and nothing points at it. |
//! | `cleanup_preserves_artifacts_before_releasing_slot` | T1 | `Rig::fixture_repo`, `Rig::pool`, `orphan` | With the release made to fail after the disposition commits, the disposition is still in the journal and its `kept` references still resolve; the slot is the only thing left behind. Order, not outcome. | The commit preceding `Lane::settle()`. Release first and a settle that fails between the two erases the only copy of a result, which section 6.6 forbids by name. |
//! | `writer_is_quiescent_before_snapshot_or_settle` | T1 | `Rig::fixture_repo`, `Rig::pool`, `git`, a background command in the lane | A lane whose child still has a command running refuses both the candidate snapshot and the settle, naming the running command; once it exits, both succeed and the snapshot covers the bytes the command wrote. | The quiescence test in `Lane::settle()`, hoisted out of `take_settled_worktree`'s `ChildStatus::Running` check. Test the status alone and a child that returned while its own background command keeps writing gets snapshotted mid-write, so the verified tree is not the tree that lands. |
//! | `user_dirty_tree_is_preserved_during_integration` and `user_staged_tree_is_preserved_during_integration` | T1 | `Rig::fixture_repo(Dirt::Dirty)` and `(Dirt::Staged)`, `git` | A full accept over a parent holding an edit to the overlapping file, unstaged or staged, and an untracked file leaves the status byte for byte and the untracked file in no commit; the candidate's new file is in the working tree and the index, and nothing of the integration shows as a staged reversal. The integration happened in a staging worktree and publication moved a ref, then followed it on every path the user had not touched. | The staging worktree, publication never running `git add -A` near the parent, and the ref-only publication restoring the untouched paths. Prepare in the user's checkout and the accept either refuses on their dirt or commits it; move the ref alone and every path the user did not touch reads as reverted, so their next `git commit -a` undoes the acceptance. |
//! | `a_replayed_done_returns_the_recorded_acceptance` | T1 | `Rig::fixture_repo`, `Bench::done_as` | The same `done` request id twice after a submit: the second answers the recorded acceptance, journals nothing, and the todo is `Done` once. | `replay` matching an `accepted` record as the `done` it answered. Compare the raw op name and a retried `done` whose reply the kernel dropped is refused `request_id_reused` for an acceptance that succeeded. |
//! | `done_naming_an_output_the_integration_never_saw_is_stale` | T1 | `Rig::fixture_repo`, `Bench::done_as`, the bench's `Serve` | `done` naming another output than the candidate submitted is refused stale with a `verification_stale` record, charges no refusal, and leaves the attempt at its verified integration; `done` naming nothing accepts the submitted output. | Section 6.3 step 5 at the accept phase: the token recomputed under the lease and compared whole. Skip it and a product the checks never saw is recorded `VerifiedDone`. |
//! | `full_worker_capacity_does_not_deadlock_verification` | T1 | `Rig::fixture_repo`, `PlanEngine::capacity` | With the engine's own worker share held to its cap, a worktree todo still submits, verifies and is accepted, and the run ends with no verification permit and no slot held. | `Purpose::Verification` being its own counter (capacity.rs). Charge the checks against the worker share and a parent whose workers hold every worker lane cannot verify the candidate any of them submits. |
//!
//! `a_repossessed_worktree_keeps_its_work_on_its_branch` landed with F2b (D215) at the end of
//! this file, against the lease journal on the parent's transcript. The plan journal's own
//! `Disposition::RepossessionPending` is still unwritten: a repossessed worktree todo reaches
//! the plan as a child the host cannot vouch for, blocks on the user, and records its
//! disposition through `fail` or `drop` as any other non-accept exit does. Writing it from
//! the repossession needs a road from the host into the engine's journal, which F3a owns.

use crate::scratch;
use crate::support;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_runtime::lane::land::{
    LaneHandle, bounded_name, format_lanes, host_of, open_pr_in, owner_repo, parse_jobs, pr_number,
};
use yi_runtime::lane::settle::{
    Held, Prepared, Publish, Published, Quiescence, Unsettled, generation_of, prepare, publish,
};
use yi_runtime::lane::{
    BranchName, ClaimBase, DEFAULT_SLOTS, Head, LaneError, Pool, SlotIndex, SlotView, TreeState,
    head, toolchain,
};
use yi_types::lane::PrNumber;

type TestResult = Result<(), Box<dyn Error>>;

/// A quiet child: nothing running under its lane.
fn quiet() -> Quiescence {
    Quiescence {
        at: 0,
        running: Vec::new(),
    }
}

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
    repo: PathBuf,
    home: PathBuf,
    root: Scratch,
}

impl Rig {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let root = Scratch::new(&format!("yi-lanes-{label}"))?;
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home)?;
        init_repo(&repo)?;
        Ok(Self { repo, home, root })
    }

    fn pool(&self, slots: u8) -> Result<Pool, Box<dyn Error>> {
        Ok(Pool::open(&self.home, &self.repo, slots)?)
    }
}

fn init_repo(repo: &Path) -> TestResult {
    std::fs::create_dir_all(repo)?;
    git(repo, &["init", "-q", "-b", "main"])?;
    git(repo, &["config", "user.email", "lanes@test"])?;
    git(repo, &["config", "user.name", "lanes"])?;
    std::fs::write(repo.join("README.md"), "base\n")?;
    git(repo, &["add", "README.md"])?;
    git(repo, &["commit", "-qm", "base"])?;
    Ok(())
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
    Ok(())
}

/// Incident: a child's candidate commit swept the `__pycache__` its checker run left behind.
#[test]
fn a_candidate_commit_leaves_build_output_out() -> TestResult {
    let rig = Rig::new("pycache")?;
    let lane = rig.pool(2)?.claim("s-py", ClaimBase::Main)?;
    std::fs::create_dir_all(lane.path().join("pkg/__pycache__"))?;
    std::fs::write(
        lane.path().join("pkg/__pycache__/mod.cpython-312.pyc"),
        "bytecode",
    )?;
    let base = git(lane.path(), &["rev-parse", "HEAD"])?;
    let noise_only = lane.candidate(&quiet(), None)?;
    assert_eq!(
        noise_only.commit.as_str(),
        base,
        "build output alone commits nothing"
    );
    std::fs::write(lane.path().join("pkg/mod.py"), "x = 1\n")?;
    let candidate = lane.candidate(&quiet(), None)?;
    let files = git(
        lane.path(),
        &[
            "show",
            "--name-only",
            "--format=",
            candidate.commit.as_str(),
        ],
    )?;
    assert_eq!(files, "pkg/mod.py");
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
    Ok(())
}

/// Incident: a console holding three sessions refused the fourth `/new` and every resume.
#[test]
fn an_unconfigured_pool_grows_past_three_live_sessions() -> TestResult {
    let rig = Rig::new("grow")?;
    let pool = rig.pool(DEFAULT_SLOTS)?;
    let lanes = ["s-1", "s-2", "s-3", "s-4"]
        .into_iter()
        .map(|session| pool.claim(session, ClaimBase::Main))
        .collect::<Result<Vec<_>, _>>()?;
    let slots: std::collections::BTreeSet<_> = lanes.iter().map(|lane| lane.slot()).collect();
    assert_eq!(slots.len(), 4, "four live sessions hold four lanes");
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
    Ok(())
}

/// Probe repos deleted and recreated at the same path left slots whose `.git` named a gitdir
/// that was gone, and every later claim in the project died on `not a git repository`.
#[test]
fn a_slot_whose_repository_was_recreated_is_a_free_slot() -> TestResult {
    let rig = Rig::new("recreated")?;
    let pool = rig.pool(2)?;
    let idle = pool.claim("s-idle", ClaimBase::Main)?;
    let left = pool.claim("pid-left", ClaimBase::Main)?;
    let slot = left.slot();
    drop((idle, left));
    git(&rig.repo, &["branch", "-q", "yi/pid-left", "main"])?;
    orphan(&pool, slot, "pid-left")?;
    let work = pool.dir().join(slot.to_string()).join("work.txt");
    std::fs::write(&work, "UNCOMMITTED\n")?;
    std::fs::remove_dir_all(&rig.repo)?;
    init_repo(&rig.repo)?;
    let pool = rig.pool(2)?;
    // Review: its own session resuming must not lose the tree, which holds the only copy.
    match pool.claim("pid-left", ClaimBase::Main) {
        Err(LaneError::RepoGone { .. }) => {}
        other => {
            return Err(format!("a resume deleted or reused its tree: {:?}", other.err()).into());
        }
    }
    assert!(
        work.is_file(),
        "the refused resume kept its uncommitted file"
    );
    let first = pool.claim("s-one", ClaimBase::Main)?;
    let second = pool.claim("s-two", ClaimBase::Main)?;
    for (lane, branch) in [(&first, "yi/s-one"), (&second, "yi/s-two")] {
        assert_eq!(
            git(lane.path(), &["symbolic-ref", "--short", "HEAD"])?,
            branch
        );
        assert_eq!(
            git(lane.path(), &["rev-parse", "HEAD"])?,
            git(&rig.repo, &["rev-parse", "main"])?,
            "the slot is a worktree of the new repository"
        );
    }
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
    let settled = child.settle(&quiet(), None)?;
    let generation = generation_of(parent.path(), None)?;
    let Prepared::Merged(staging) = prepare(
        &pool,
        "stage-sub-1",
        &generation,
        &settled.candidate.commit,
        None,
    )?
    else {
        return Err("a clean candidate conflicted".into());
    };
    let published = publish(
        parent.path(),
        &generation.base,
        None,
        &staging.integrated,
        None,
    )?;
    staging.release()?;
    assert!(
        matches!(published, Publish::Published { .. }),
        "{published:?}"
    );
    assert!(
        parent.path().join("child.txt").is_file(),
        "the merge landed in the parent's lane"
    );
    assert!(
        !rig.repo.join("child.txt").exists(),
        "the trunk checkout is never the merge target"
    );
    drop(parent);
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
    Ok(())
}

/// `fgj api` takes the host spelled out, read from the origin in either spelling.
#[test]
fn the_origin_host_is_read_from_ssh_and_https_urls() {
    assert_eq!(
        host_of("ssh://git@forge.example.invalid:2222/apex/yi.git").as_deref(),
        Some("forge.example.invalid")
    );
    assert_eq!(
        host_of("git@forge.example.invalid:apex/yi.git").as_deref(),
        Some("forge.example.invalid")
    );
    assert_eq!(
        host_of("https://git.example.invalid/apex/yi").as_deref(),
        Some("git.example.invalid")
    );
    assert_eq!(host_of(""), None);
}

/// The forge's open list is read back to a typed number by head branch, on either forge.
#[test]
fn an_open_pull_request_is_found_by_head() {
    let fgj = r#"[{"number": 12, "head": {"ref": "yi/other"}}, {"number": 191, "head": {"ref": "yi/01a0"}}]"#;
    assert_eq!(open_pr_in(fgj, "yi/01a0"), Some(PrNumber(191)));
    assert_eq!(open_pr_in(fgj, "yi/none"), None);
    assert_eq!(
        open_pr_in(r#"[{"number": 7}]"#, "yi/01a0"),
        Some(PrNumber(7))
    );
    assert_eq!(open_pr_in("[]", "yi/01a0"), None);
    assert_eq!(open_pr_in("garbage", "yi/01a0"), None);
}

/// The user's uncommitted work in the fixture parent, or none (`repo.sh`'s second argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dirt {
    Clean,
    Dirty,
    Staged,
    /// An untracked `docs/` holding the user's own file at the path the green candidate adds.
    Nested,
}

impl Dirt {
    fn arg(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Dirty => "dirty",
            Self::Staged => "staged",
            Self::Nested => "nested",
        }
    }
}

impl Rig {
    /// A rig whose repository is the section 6.6 fixture parent, returned beside it: the
    /// scratch repository `Rig::new` commits would be five git runs no fixture test reads.
    fn fixture(label: &str, dirt: Dirt) -> Result<(Self, PathBuf), Box<dyn Error>> {
        let root = Scratch::new(&format!("yi-lanes-{label}"))?;
        let home = root.join("home");
        std::fs::create_dir_all(&home)?;
        let mut rig = Self {
            repo: PathBuf::new(),
            home,
            root,
        };
        rig.repo = rig.fixture_repo(dirt)?;
        let parent = rig.repo.clone();
        Ok((rig, parent))
    }

    /// Build the section 6.6 fixture repository under this rig's scratch and return the parent
    /// checkout. `dirt` leaves the user's uncommitted work in it.
    fn fixture_repo(&self, dirt: Dirt) -> Result<PathBuf, Box<dyn Error>> {
        let dir = self.root.join("fixture");
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/worktree/repo.sh");
        let output = yi_tools::command("sh")
            .arg(&script)
            .arg(&dir)
            .arg(dirt.arg())
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "repo.sh failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(dir.join("parent"))
    }
}

fn sha(cwd: &Path, refname: &str) -> Result<yi_runtime::lane::Sha, Box<dyn Error>> {
    let text = git(
        cwd,
        &["rev-parse", "--verify", &format!("{refname}^{{commit}}")],
    )?;
    yi_runtime::lane::Sha::parse(&text).ok_or_else(|| format!("{refname}: not a sha").into())
}

/// What the parent checkout looks like: head, porcelain status, and the two files the dirt
/// touches, so an integration can be checked byte for byte.
fn parent_view(parent: &Path) -> Result<(String, String, String, String), Box<dyn Error>> {
    Ok((
        git(parent, &["rev-parse", "HEAD"])?,
        git(parent, &["status", "--porcelain"])?,
        std::fs::read_to_string(parent.join("rotate.sh"))?,
        std::fs::read_to_string(parent.join("scratch.txt")).unwrap_or_default(),
    ))
}

fn held(pool: &Pool) -> Result<usize, Box<dyn Error>> {
    Ok(pool
        .list()?
        .iter()
        .filter(|view| matches!(view, SlotView::Held { .. }))
        .count())
}

/// Plan section 6.6 driven through the engine against the fixture repository: the child is a
/// fixture branch, the checker is the frozen `grep`, the parent is the fixture checkout.
mod accept {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use yi_runtime::lane::settle::Candidate;
    use yi_runtime::plan::acceptance::{
        Candidate as Proven, KIND_ACCEPTED, KIND_CANDIDATE_SUBMITTED, KIND_DISPOSITION,
        KIND_INTEGRATION_INTENT, KIND_INTEGRATION_PREPARED, KIND_INTEGRATION_STALE,
        KIND_INTEGRATION_VERIFIED, Phase, Quiescence, Verified, phase_of,
    };
    use yi_runtime::plan::capacity::Purpose;
    use yi_runtime::plan::journal::{Journal, RealFs};
    use yi_runtime::plan::ops::{
        Actor, Delegate, Op, OpRequest, Outcome, OutputResolve, PlanEngine, PlanOpError,
    };
    use yi_runtime::plan::store::PlanStore;
    use yi_runtime::plan::verify::Verifier;
    use yi_types::plan::contract::Contract;
    use yi_types::plan::doc::{
        AgentId, Check, Delegation, GoalText, Isolation, PlanId, SpawnSpec, TodoAddr, TodoLabel,
        TodoState,
    };
    use yi_types::plan::ledger::{AttemptId, JournalRecord, RequestId};
    use yi_types::plan::op::{Choice, TodoSpec};
    use yi_types::subagent::ChildExit;
    use yi_types::url::Url;

    pub(super) const LABEL: &str = "add rotate";
    const GOAL: &str = "land the rotate subcommand";
    /// The frozen checker: the candidate and the merged tree both list `rotate`. It writes a
    /// file into the tree it checks, as a build or a test run would: nothing of that is the
    /// integration, and nothing may commit it.
    const CHECK: &str = "touch checker-droppings.txt; grep -q 'subcommands: list rotate' rotate.sh";

    /// Every output resolves, to a text naming its url: two outputs never share a digest.
    struct Serve;

    impl OutputResolve for Serve {
        fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
            Ok(Some(format!("served {url}")))
        }
    }

    /// A child that is a fixture branch: no process, no lane of its own, a candidate the
    /// engine's dispose seam hands back, and a reap that can be made to fail.
    pub(super) struct Child {
        candidate: Mutex<Option<Candidate>>,
        /// The checkout a `local://` output is read in: the fixture parent, since the fixture
        /// branch has no lane of its own.
        path: PathBuf,
        pub(super) fail_reap: AtomicBool,
        pub(super) disposed: Mutex<Vec<Choice>>,
        /// What each spawn was handed, the engine's brief lines included.
        pub(super) briefs: Mutex<Vec<Delegation>>,
    }

    impl Delegate for Child {
        fn spawn(&self, _at: &TodoAddr, delegation: &Delegation) -> Result<AgentId, String> {
            if let Ok(mut briefs) = self.briefs.lock() {
                briefs.push(delegation.clone());
            }
            AgentId::new("child-1").map_err(|error| error.to_string())
        }

        fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
            if self.fail_reap.load(Ordering::SeqCst) {
                return Err("the child is wedged".to_owned());
            }
            Ok(None)
        }

        fn candidate(&self, _agent: &AgentId) -> Result<Option<Held>, String> {
            let candidate = self.candidate.lock().map_err(|_| "poisoned")?.clone();
            Ok(candidate.map(|candidate| Held {
                candidate,
                quiescence: Quiescence {
                    at: 0,
                    running: Vec::new(),
                },
                path: self.path.clone(),
            }))
        }

        fn mark(&self, _agent: &AgentId, choice: Choice) -> Result<(), String> {
            if let Ok(mut disposed) = self.disposed.lock() {
                disposed.push(choice);
            }
            Ok(())
        }
    }

    pub(super) struct Bench {
        pub(super) store: PlanStore,
        pub(super) engine: PlanEngine,
        pub(super) child: Arc<Child>,
        pub(super) plan: PlanId,
        pub(super) parent: PathBuf,
        pub(super) pool: Pool,
    }

    /// The engine over the fixture parent with one started worktree todo whose child stands
    /// on `branch`.
    pub(super) fn bench(rig: &Rig, parent: PathBuf, branch: &str) -> Result<Bench, Box<dyn Error>> {
        let pool = Pool::open(&rig.home, &parent, 3)?;
        let store = PlanStore::open(rig.root.join("plans"))?;
        let plan = PlanId::slug(GOAL)?;
        let manifest = json!({
            "manifest": 1, "command": CHECK, "cwd": "snapshot_root", "cwd_subdir": null,
            "protected": [], "timeout_ms": 10_000, "env": [], "reads_outside_snapshot": false
        });
        let checker = store.artifacts(&plan).put(
            &serde_json::to_vec(&manifest)?,
            "application/vnd.yi.checker-manifest+json",
            &store.nonce(),
        )?;
        let contract: Contract = serde_json::from_value(json!({
            "class": "writer",
            "items": [{"id": "rotate-listed", "critical": true, "weight": 100,
                       "decider": {"cmd": {"checker": checker, "timeout_ms": 10_000}}}],
            "threshold": 1000, "min_coverage": 1000
        }))?;
        let candidate = Candidate {
            branch: BranchName::parse(branch)?,
            commit: sha(&parent, branch)?,
            base: Some(sha(&parent, "main")?),
        };
        let child = Arc::new(Child {
            candidate: Mutex::new(Some(candidate)),
            path: parent.clone(),
            fail_reap: AtomicBool::new(false),
            disposed: Mutex::new(Vec::new()),
            briefs: Mutex::new(Vec::new()),
        });
        let engine = PlanEngine::new(store.clone(), child.clone())
            .with_cwd(parent.clone())
            .with_output_resolve(Arc::new(Serve))
            .with_verifier(Verifier::new(20_000))
            .with_lanes(pool.clone());
        let delegation = Delegation {
            spec: SpawnSpec {
                role: None,
                model: None,
                effort: None,
                tools: Vec::new(),
                isolation: Some(Isolation::Worktree),
                budget: None,
                wall: None,
                parent_close: None,
                extra: serde_json::Map::new(),
            },
            accept: Check::Command("true".to_owned()),
            output: None,
            context: Vec::new(),
            note: None,
            extra: serde_json::Map::new(),
        };
        let opened = engine.apply(OpRequest {
            plan: None,
            actor: Actor::Owner,
            op: Op::Init {
                goal: GoalText::new(GOAL)?,
                todos: vec![TodoSpec {
                    label: TodoLabel::new(LABEL)?,
                    after: Vec::new(),
                    delegation: Some(delegation),
                    contract: Some(contract),
                    children: Vec::new(),
                    cites: Default::default(),
                }],
            },
            request_id: None,
            expected_revision: None,
        })?;
        let bench = Bench {
            store,
            engine,
            child,
            plan,
            parent,
            pool,
        };
        // The engine starts the worktree todo with the init; no owner op spawns it.
        if opened.spawned.len() != 1 {
            return Err(format!("the engine did not start {LABEL}: {:?}", opened.notices).into());
        }
        Ok(bench)
    }

    impl Bench {
        pub(super) fn owner(&self, op: Op) -> Result<Outcome, PlanOpError> {
            self.engine.apply(OpRequest {
                plan: Some(self.plan.clone()),
                actor: Actor::Owner,
                op,
                request_id: None,
                expected_revision: None,
            })
        }

        /// The child's own `submit`: rows one to three of section 6.6.
        pub(super) fn submit(&self) -> Result<Outcome, PlanOpError> {
            self.engine.apply(OpRequest {
                plan: Some(self.plan.clone()),
                actor: Actor::Child(AgentId::new("child-1").map_err(PlanOpError::Doc)?),
                op: Op::Submit {
                    label: TodoLabel::new(LABEL).map_err(PlanOpError::Doc)?,
                    attempt: AttemptId::FIRST,
                    output: "local://rotate.sh"
                        .parse()
                        .map_err(|_| PlanOpError::NoPlan)?,
                },
                request_id: None,
                expected_revision: None,
            })
        }

        pub(super) fn done(&self) -> Result<Outcome, PlanOpError> {
            self.done_as(None, None)
        }

        /// `done` under a request id of the caller's, naming an output or not.
        pub(super) fn done_as(
            &self,
            request: Option<&str>,
            output: Option<&str>,
        ) -> Result<Outcome, PlanOpError> {
            let output = match output {
                Some(url) => Some(url.parse().map_err(|_| PlanOpError::NoPlan)?),
                None => None,
            };
            let request_id = match request {
                Some(id) => Some(RequestId::new(id).map_err(|_| PlanOpError::NoPlan)?),
                None => None,
            };
            self.engine.apply(OpRequest {
                plan: Some(self.plan.clone()),
                actor: Actor::Owner,
                op: Op::Done {
                    label: TodoLabel::new(LABEL).map_err(PlanOpError::Doc)?,
                    output,
                },
                request_id,
                expected_revision: None,
            })
        }

        pub(super) fn fail(&self, disposition: Option<Choice>) -> Result<Outcome, PlanOpError> {
            self.owner(Op::Fail {
                label: TodoLabel::new(LABEL).map_err(PlanOpError::Doc)?,
                cause: "the owner failed it".to_owned(),
                disposition,
            })
        }

        pub(super) fn records(&self) -> Result<Vec<JournalRecord>, Box<dyn Error>> {
            let journal = Journal::open(self.store.journal_path(&self.plan), Arc::new(RealFs));
            Ok(journal.read()?.records)
        }

        pub(super) fn kinds(&self) -> Result<Vec<String>, Box<dyn Error>> {
            Ok(self
                .records()?
                .iter()
                .map(|record| record.record.op.clone())
                .collect())
        }

        pub(super) fn state(&self) -> Result<TodoState, Box<dyn Error>> {
            Ok(self
                .store
                .read(&self.plan)?
                .todo(&TodoLabel::new(LABEL)?)
                .map(|todo| todo.state.clone())
                .ok_or("todo missing")?)
        }

        pub(super) fn stage_branches(&self) -> Result<String, Box<dyn Error>> {
            git(&self.parent, &["branch", "--list", "yi/stage-*"])
        }
    }

    /// One journal record appended behind the engine's back, the way a crashed process would
    /// leave it: sealed onto the chain, never reduced by this call.
    fn seed(bench: &Bench, kind: &str, args: serde_json::Value) -> TestResult {
        let journal = bench.store.journal(&bench.plan);
        let last = journal
            .read()?
            .records
            .last()
            .cloned()
            .ok_or("empty journal")?;
        let mut value = serde_json::to_value(&last)?;
        value["op"] = json!(kind);
        value["todo"] = json!(LABEL);
        value["requestId"] = json!(format!("seed-{kind}-{}", bench.store.nonce()));
        value["attempt"] = json!(1);
        value["args"] = args;
        if let Some(fields) = value.as_object_mut() {
            for gone in ["from", "to", "verdict", "effect_id", "resolution"] {
                fields.remove(gone);
            }
        }
        let record: JournalRecord = serde_json::from_value(value)?;
        journal.append(&journal.seal(record, Some(&last))?)?;
        Ok(())
    }

    fn refusals(bench: &Bench) -> Result<u32, Box<dyn Error>> {
        let plan = bench.store.read(&bench.plan)?;
        Ok(plan
            .todo(&TodoLabel::new(LABEL)?)
            .ok_or("todo missing")?
            .refusals)
    }

    // Dies with the order in `submit_candidate` (acceptance.rs): integrate before the
    // candidate verdict and an `integration_prepared` record and a stage branch appear for a
    // candidate the checker refused.
    #[test]
    fn failing_candidate_never_contaminates_parent_checkout() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-red", Dirt::Dirty)?;
        let before = parent_view(&parent)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-red")?;
        let refused = bench.submit();
        assert!(
            matches!(refused, Err(PlanOpError::Refused { .. })),
            "{refused:?}"
        );
        let kinds = bench.kinds()?;
        assert!(
            kinds.contains(&KIND_CANDIDATE_SUBMITTED.to_owned()),
            "{kinds:?}"
        );
        assert!(kinds.contains(&"done_refused".to_owned()), "{kinds:?}");
        assert!(
            !kinds.contains(&KIND_INTEGRATION_PREPARED.to_owned()),
            "no merge is prepared for a refused candidate: {kinds:?}"
        );
        assert_eq!(parent_view(&parent)?, before, "the parent is untouched");
        assert!(
            sha(&parent, "yi/cand-red").is_ok(),
            "the candidate is still readable"
        );
        assert_eq!(bench.stage_branches()?, "", "no staging branch was cut");
        assert_eq!(held(&bench.pool)?, 0, "no slot is held");
        assert!(matches!(bench.state()?, TodoState::Running { .. }));
        // Dies with the settled-verdict replay in `submit_prepare`: a second submit of the
        // same token runs the checker again and charges a second refusal.
        assert_eq!(refusals(&bench)?, 1);
        let again = bench.submit();
        assert!(
            matches!(again, Err(PlanOpError::Refused { .. })),
            "{again:?}"
        );
        let kinds = bench.kinds()?;
        let count = |kind: &str| kinds.iter().filter(|k| k.as_str() == kind).count();
        assert_eq!(count("verification_requested"), 1, "{kinds:?}");
        assert_eq!(count("done_refused"), 1, "{kinds:?}");
        assert_eq!(refusals(&bench)?, 1, "the replay charges nothing");
        Ok(())
    }

    // Dies with `-uall` on the status read in `publish` (settle.rs): `git status` collapses an
    // untracked directory to `docs/`, so `docs/ROTATE.md` reads as untouched and the restore
    // writes the integration's bytes over the user's only copy.
    #[test]
    fn an_untracked_directory_survives_a_ref_only_publication() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-nested", Dirt::Nested)?;
        let mine = std::fs::read_to_string(parent.join("docs/ROTATE.md"))?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        bench.done()?;
        let records = bench.records()?;
        let accepted = records
            .iter()
            .find(|record| record.record.op == KIND_ACCEPTED)
            .ok_or("no accepted record")?;
        assert_eq!(accepted.args["how"], "ref_only");
        assert_eq!(
            std::fs::read_to_string(parent.join("docs/ROTATE.md"))?,
            mine,
            "the user's file is left byte for byte"
        );
        assert!(
            git(&parent, &["show", "HEAD:docs/ROTATE.md"])?.contains("Rotates the logs"),
            "the published commit carries the candidate's file"
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        Ok(())
    }

    // Dies with the toplevel resolution in `publish` (settle.rs): run it from the session's
    // cwd and `restore -- docs/ROTATE.md` resolves against `sub/` after the ref has moved.
    #[test]
    fn a_publication_runs_from_the_repository_root_whatever_the_cwd() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-subdir", Dirt::Dirty)?;
        let pool = Pool::open(&rig.home, &parent, 2)?;
        let generation = generation_of(&parent, None)?;
        let Prepared::Merged(staging) = prepare(
            &pool,
            "stage-sub",
            &generation,
            &sha(&parent, "yi/cand-green")?,
            None,
        )?
        else {
            return Err("the green candidate conflicted".into());
        };
        let sub = parent.join("sub");
        std::fs::create_dir(&sub)?;
        let published = publish(&sub, &generation.base, None, &staging.integrated, None)?;
        assert!(
            matches!(
                &published,
                Publish::Published {
                    how: Published::RefOnly,
                    ..
                }
            ),
            "{published:?}"
        );
        assert_eq!(sha(&parent, "HEAD")?, staging.integrated);
        assert!(parent.join("docs/ROTATE.md").is_file());
        assert!(std::fs::read_to_string(parent.join("rotate.sh"))?.contains("staged by nobody"));
        staging.release()?;
        Ok(())
    }

    // Dies with the dirt check before `update-ref` in `publish` (settle.rs): take the ref-only
    // path on any fast-forward failure and a lock or an unconcluded merge moves the branch
    // with the checkout left behind.
    #[test]
    fn a_transient_fast_forward_failure_publishes_nothing() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-transient", Dirt::Dirty)?;
        let pool = Pool::open(&rig.home, &parent, 2)?;
        let generation = generation_of(&parent, None)?;
        let Prepared::Merged(staging) = prepare(
            &pool,
            "stage-lock",
            &generation,
            &sha(&parent, "yi/cand-green")?,
            None,
        )?
        else {
            return Err("the green candidate conflicted".into());
        };
        let before = parent_view(&parent)?;
        for blocker in ["index.lock", "MERGE_HEAD"] {
            let path = parent.join(".git").join(blocker);
            std::fs::write(&path, format!("{}\n", generation.base.as_str()))?;
            let refused = publish(&parent, &generation.base, None, &staging.integrated, None);
            assert!(
                matches!(refused, Err(LaneError::Git { .. })),
                "{blocker}: {refused:?}"
            );
            assert_eq!(parent_view(&parent)?, before, "{blocker}: nothing moved");
            std::fs::remove_file(&path)?;
        }
        let published = publish(&parent, &generation.base, None, &staging.integrated, None)?;
        assert!(
            matches!(
                &published,
                Publish::Published {
                    how: Published::RefOnly,
                    ..
                }
            ),
            "{published:?}"
        );
        staging.release()?;
        Ok(())
    }

    // Dies with `Phase::IntegrationStale` in `try_publish` (acceptance.rs): map the stale
    // record back to the verified candidate and the `done` after a crashed re-preparation is
    // refused `integration_prepared` missing, with no way back but `retry`.
    #[test]
    fn a_stale_integration_left_unprepared_is_prepared_by_the_next_done() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-stale-crash", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let old = sha(&parent, "HEAD")?;
        std::fs::write(parent.join("NEWS.md"), "moved on\n")?;
        git(&parent, &["add", "NEWS.md"])?;
        git(&parent, &["commit", "-qm", "the parent moved"])?;
        let moved = sha(&parent, "HEAD")?;
        let integrated = bench
            .records()?
            .iter()
            .rev()
            .find(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .and_then(|record| record.args["integrated"].as_str().map(str::to_owned))
            .ok_or("no integration_prepared record")?;
        // A predecessor recorded the stale publication and died before preparing again.
        seed(
            &bench,
            KIND_INTEGRATION_STALE,
            json!({"label": LABEL, "expected": old.as_str(), "found": moved.as_str(),
                   "integrated": integrated}),
        )?;
        bench.done()?;
        let generations: Vec<u64> = bench
            .records()?
            .iter()
            .filter(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .filter_map(|record| record.args["generation"].as_u64())
            .collect();
        assert_eq!(generations, vec![1, 2]);
        assert_eq!(
            bench.kinds()?.last().map(String::as_str),
            Some(KIND_ACCEPTED)
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        assert!(parent.join("NEWS.md").is_file());
        assert_eq!(held(&bench.pool)?, 0);
        Ok(())
    }

    // Dies with the `found == integrated` arm in `try_publish` (acceptance.rs): a run killed
    // after the publication and before `accepted` reads its own HEAD as a moved parent.
    #[test]
    fn a_publication_killed_before_its_record_is_accepted_by_the_next_done() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-publish-crash", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let prepared = bench
            .records()?
            .into_iter()
            .rev()
            .find(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .ok_or("no integration_prepared record")?;
        let integrated = sha(
            &parent,
            prepared.args["integrated"].as_str().ok_or("integrated")?,
        )?;
        let base = sha(
            &parent,
            prepared.args["parent_base"].as_str().ok_or("base")?,
        )?;
        // The killed run's publication: the parent's ref moved and no record followed.
        let branch = prepared.args["parent_branch"].as_str();
        publish(&parent, &base, branch, &integrated, None)?;
        bench.done()?;
        let kinds = bench.kinds()?;
        let count = |kind: &str| kinds.iter().filter(|k| k.as_str() == kind).count();
        assert_eq!(count(KIND_INTEGRATION_STALE), 0, "{kinds:?}");
        assert_eq!(count(KIND_INTEGRATION_PREPARED), 1, "{kinds:?}");
        assert_eq!(kinds.last().map(String::as_str), Some(KIND_ACCEPTED));
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        assert_eq!(sha(&parent, "HEAD")?, integrated);
        assert_eq!(
            bench.stage_branches()?,
            "",
            "the pin went with the acceptance"
        );
        Ok(())
    }

    // Dies with `stage_intent` (submit.rs): a run killed holding its staging lane before
    // `integration_prepared` leaves a slot and a branch no record names and no `done` frees.
    #[test]
    fn a_staging_lane_killed_before_its_record_is_dropped_by_the_next_done() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-stage-crash", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let kinds = bench.kinds()?;
        let at = |kind: &str| kinds.iter().position(|k| k.as_str() == kind);
        assert!(
            at(KIND_INTEGRATION_INTENT).is_some_and(|i| Some(i) < at(KIND_INTEGRATION_PREPARED)),
            "the staging session is journaled before it is claimed: {kinds:?}"
        );
        let head = sha(&parent, "HEAD")?;
        // A predecessor found the parent moved, journaled its staging session, claimed the
        // lane, merged into it and died before `integration_prepared`.
        seed(
            &bench,
            KIND_INTEGRATION_STALE,
            json!({"label": LABEL, "expected": head.as_str(), "found": head.as_str(),
                   "integrated": head.as_str()}),
        )?;
        seed(
            &bench,
            KIND_INTEGRATION_INTENT,
            json!({"label": LABEL, "generation": 2, "staging": "stage-lost"}),
        )?;
        let lane = bench
            .pool
            .claim("stage-lost", ClaimBase::Commit(head.as_str().to_owned()))?;
        git(
            lane.path(),
            &["merge", "-q", "--no-ff", "--no-edit", "yi/cand-green"],
        )?;
        let slot = lane.slot();
        drop(lane);
        orphan(&bench.pool, slot, "stage-lost")?;
        bench.done()?;
        assert_eq!(git(&parent, &["branch", "--list", "yi/stage-lost"])?, "");
        assert!(
            !bench
                .pool
                .list()?
                .iter()
                .any(|view| matches!(view, SlotView::Orphan { .. })),
            "the killed run's slot is free"
        );
        let abandoned = bench
            .records()?
            .into_iter()
            .rev()
            .find(|record| record.record.op == KIND_INTEGRATION_INTENT)
            .map(|record| record.args["abandoned"].clone())
            .ok_or("no integration_intent record")?;
        assert_eq!(
            abandoned,
            json!({"staging": "stage-lost", "pin": {"dropped": true}})
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        Ok(())
    }

    // Dies with `repair` blind to staging: a crash between `integration_intent` and
    // `integration_prepared` left a slot and a branch that only a retry of that attempt freed.
    #[test]
    fn repair_drops_the_staging_a_crash_left_before_prepare_and_names_it() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-stage-repair", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let head = sha(&parent, "HEAD")?;
        seed(
            &bench,
            KIND_INTEGRATION_INTENT,
            json!({"label": LABEL, "generation": 2, "staging": "stage-lost"}),
        )?;
        let lane = bench
            .pool
            .claim("stage-lost", ClaimBase::Commit(head.as_str().to_owned()))?;
        git(
            lane.path(),
            &["merge", "-q", "--no-ff", "--no-edit", "yi/cand-green"],
        )?;
        let slot = lane.slot();
        drop(lane);
        orphan(&bench.pool, slot, "stage-lost")?;
        let repair = || {
            bench.owner(Op::Repair {
                resolutions: Vec::new(),
            })
        };
        let said = repair()?.notices.join("\n");
        assert!(said.contains("dropped staging stage-lost"), "{said}");
        assert_eq!(git(&parent, &["branch", "--list", "yi/stage-lost"])?, "");
        let again = repair()?.notices.join("\n");
        assert!(!again.contains("stage-lost"), "{again}");
        Ok(())
    }

    // Dies with `Phase::IntegrationPrepared` handled as a re-preparation in `try_publish`
    // (acceptance.rs): treat it as a missing record instead and a verified candidate whose
    // process died between `integration_prepared` and `integration_verified` is refused
    // `PhaseMissing` on every later `done`, its passing verdict thrown away.
    #[test]
    fn a_verified_candidate_whose_integration_never_landed_is_prepared_by_the_next_done()
    -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-prepared-crash", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let base = sha(&parent, "HEAD")?;
        let integrated = bench
            .records()?
            .iter()
            .rev()
            .find(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .and_then(|record| record.args["integrated"].as_str().map(str::to_owned))
            .ok_or("no integration_prepared record")?;
        // A predecessor prepared generation 2 and died before verifying it.
        seed(
            &bench,
            KIND_INTEGRATION_PREPARED,
            json!({"label": LABEL, "candidate": integrated, "parent_base": base.as_str(),
                   "generation": 2, "integrated": integrated, "staging": null,
                   "conflict_provenance": []}),
        )?;
        bench.done()?;
        let generations: Vec<u64> = bench
            .records()?
            .iter()
            .filter(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .filter_map(|record| record.args["generation"].as_u64())
            .collect();
        assert_eq!(generations, vec![1, 2, 3]);
        assert_eq!(
            bench.kinds()?.last().map(String::as_str),
            Some(KIND_ACCEPTED)
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        Ok(())
    }

    // Dies with `is_worktree_todo` propagating its read error (acceptance.rs): swallow it
    // into `false` and a worktree todo whose checkpoint cannot be read takes the plain done
    // path, where nothing asks for a candidate, an integration or an `accepted` record.
    #[test]
    fn an_unreadable_checkpoint_never_completes_a_worktree_todo_plainly() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-unreadable", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        std::fs::write(
            rig.root
                .join("plans")
                .join(bench.plan.as_str())
                .join("plan.json"),
            "{ not a checkpoint",
        )?;
        let refused = bench.done();
        assert!(
            refused.is_err(),
            "an unreadable checkpoint refuses done: {refused:?}"
        );
        let kinds = bench.kinds()?;
        assert!(
            !kinds
                .iter()
                .any(|kind| kind == KIND_ACCEPTED || kind == "done"),
            "nothing completed the todo: {kinds:?}"
        );
        Ok(())
    }

    // Dies with the record order in `Candidate::<Verified>::from_records` (acceptance.rs):
    // pair the last submission with any verdict and a re-submitted commit borrows the verdict
    // of the tree it replaced.
    #[test]
    fn a_resubmitted_candidate_carries_no_verdict_from_the_one_it_replaced() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-resubmit", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let label = TodoLabel::new(LABEL)?;
        let before = bench.records()?;
        assert!(
            Proven::<Verified>::from_records(&before, &bench.plan, &label, AttemptId::FIRST)
                .is_some(),
            "the submitted candidate is verified"
        );
        let moved = sha(&parent, "main")?;
        seed(
            &bench,
            KIND_CANDIDATE_SUBMITTED,
            json!({"label": LABEL, "branch": "yi/cand-green", "candidate": moved.as_str(),
                   "parent_base": moved.as_str(), "outputs": ["local://rotate.sh"],
                   "quiescent": {"at": 0, "running_commands": 0}}),
        )?;
        let after = bench.records()?;
        assert!(
            Proven::<Verified>::from_records(&after, &bench.plan, &label, AttemptId::FIRST)
                .is_none(),
            "a new submission has no verdict until its own tree is checked"
        );
        Ok(())
    }

    // Dies with the expected base compared in `publish` (settle.rs): drop it and a parent that
    // moved makes `done` fail on git instead of preparing again against the new generation.
    #[test]
    fn changed_parent_generation_rejects_stale_integration() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-stale", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        assert_eq!(
            bench.kinds()?.last().map(String::as_str),
            Some(KIND_INTEGRATION_VERIFIED)
        );
        let old = sha(&parent, "HEAD")?;
        std::fs::write(parent.join("NEWS.md"), "moved on\n")?;
        git(&parent, &["add", "NEWS.md"])?;
        git(&parent, &["commit", "-qm", "the parent moved"])?;
        let moved = sha(&parent, "HEAD")?;
        bench.done()?;
        let records = bench.records()?;
        let stale = records
            .iter()
            .find(|record| record.record.op == KIND_INTEGRATION_STALE)
            .ok_or("no integration_stale record")?;
        assert_eq!(stale.args["expected"], old.as_str());
        assert_eq!(stale.args["found"], moved.as_str());
        let generations: Vec<u64> = records
            .iter()
            .filter(|record| record.record.op == KIND_INTEGRATION_PREPARED)
            .filter_map(|record| record.args["generation"].as_u64())
            .collect();
        assert_eq!(
            generations,
            vec![1, 2],
            "prepared again against the new generation"
        );
        assert_eq!(
            bench.kinds()?.last().map(String::as_str),
            Some(KIND_ACCEPTED)
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        assert!(
            parent.join("NEWS.md").is_file(),
            "the moved generation is in the result"
        );
        assert!(
            std::fs::read_to_string(parent.join("rotate.sh"))?.contains("list rotate"),
            "the candidate is in the result"
        );
        assert_eq!(
            bench.stage_branches()?,
            "",
            "the pins went with the publication"
        );
        assert_eq!(held(&bench.pool)?, 0);
        Ok(())
    }

    // Dies with the choice riding the op in `dispose` (acceptance.rs): map `Discarded` to the
    // default and the record says retained for a branch the host deletes.
    #[test]
    fn discarded_worktree_is_not_reported_as_merged() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-discard", Dirt::Clean)?;
        let head = sha(&parent, "HEAD")?;
        let bench = bench(&rig, parent.clone(), "yi/cand-red")?;
        bench.fail(Some(Choice::Discarded))?;
        let records = bench.records()?;
        let disposition = records
            .iter()
            .find(|record| record.record.op == KIND_DISPOSITION)
            .ok_or("no disposition record")?;
        let discarded = &disposition.args["disposition"]["discarded"];
        assert_eq!(discarded["branch"], "yi/cand-red");
        assert_eq!(
            discarded["candidate"],
            sha(&parent, "yi/cand-red")?.as_str()
        );
        assert_eq!(
            discarded["kept"][0], "history://child-1",
            "the pin lands first"
        );
        assert!(
            !bench.kinds()?.contains(&KIND_ACCEPTED.to_owned()),
            "a discard is never an acceptance"
        );
        assert_eq!(
            bench.child.disposed.lock().map_err(|_| "poisoned")?[..],
            [Choice::Discarded]
        );
        assert_eq!(sha(&parent, "HEAD")?, head, "the parent did not move");
        assert!(matches!(bench.state()?, TodoState::Failed { .. }));
        // The lane half: a settled lane discarded answers with its candidate, not a merge.
        let lane = bench
            .pool
            .claim("spike", ClaimBase::Commit(head.as_str().to_owned()))?;
        std::fs::write(lane.path().join("spike.txt"), "throwaway\n")?;
        let settled = lane.settle(&quiet(), None)?;
        let branch = settled.candidate.branch.clone();
        let candidate = settled.discard()?;
        assert!(
            git(&parent, &["rev-parse", "--verify", branch.as_str()]).is_err(),
            "the branch is gone"
        );
        assert!(
            git(&parent, &["cat-file", "-e", candidate.commit.as_str()]).is_ok(),
            "the candidate commit still resolves"
        );
        assert!(!parent.join("spike.txt").exists(), "nothing crossed back");
        // A child that committed nothing left its tip on main, so `settle` deleted the branch
        // already; the discard is still the outcome asked for, not a failed reap.
        let lane = bench
            .pool
            .claim("noop", ClaimBase::Commit(head.as_str().to_owned()))?;
        let settled = lane.settle(&quiet(), None)?;
        let branch = settled.candidate.branch.clone();
        assert!(
            git(&parent, &["rev-parse", "--verify", branch.as_str()]).is_err(),
            "the settle deleted a branch whose tip is on main"
        );
        let candidate = settled.discard()?;
        assert_eq!(candidate.commit, head);
        assert_eq!(held(&bench.pool)?, 0);
        Ok(())
    }

    // Dies with the accepting road skipping `mark` in `reap_leaving` (acceptance.rs): the host
    // reads the child as holding an undisposed worktree and refuses the reap `done` runs, after
    // the integration is already published.
    #[test]
    fn an_accepted_worktree_tells_the_host_its_branch_is_kept() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-accept-mark", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        bench.done()?;
        assert!(
            bench.kinds()?.contains(&KIND_ACCEPTED.to_owned()),
            "the candidate was accepted"
        );
        assert_eq!(
            bench.child.disposed.lock().map_err(|_| "poisoned")?[..],
            [Choice::Retained],
            "the host is told the accepted branch is kept, so its reap settles the lane"
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        Ok(())
    }

    /// The child's finish as the host hands it over, with the answer it left.
    fn finish(bench: &Bench) -> Option<String> {
        bench.engine.finish_child(
            "child-1",
            ChildExit::Completed,
            None,
            Some("rotate is listed".to_owned()),
        )
    }

    fn actors(bench: &Bench, kind: &str) -> Result<Vec<String>, Box<dyn Error>> {
        Ok(bench
            .records()?
            .iter()
            .filter(|record| record.record.op == kind)
            .map(|record| record.record.actor.clone())
            .collect())
    }

    // Dies with the submit step in `accept_finish` (finish.rs): the engine's `done` finds no
    // candidate and the todo stays running under a child that ended.
    #[test]
    fn a_finished_worktree_child_is_accepted_without_an_owner_op() -> TestResult {
        let (rig, parent) = Rig::fixture("g2-accept", Dirt::Clean)?;
        let bench = bench(&rig, parent, "yi/cand-green")?;
        let line = finish(&bench).ok_or("the engine did not take its own child")?;
        assert!(
            line.starts_with("plan: accepted \"add rotate\" (agent://"),
            "{line}"
        );
        assert!(
            matches!(bench.state()?, TodoState::Done { .. }),
            "{:?}",
            bench.state()?
        );
        assert_eq!(actors(&bench, KIND_CANDIDATE_SUBMITTED)?, ["engine"]);
        assert_eq!(actors(&bench, KIND_ACCEPTED)?, ["engine"]);
        assert!(
            bench
                .engine
                .finish_child("child-1", ChildExit::Completed, None, None)
                .is_none(),
            "a todo no longer running is nobody's finish"
        );
        Ok(())
    }

    // Dies with the phase read in `locate` (finish.rs): a child that submitted its own
    // candidate gets a second submission from the engine and a second verification.
    #[test]
    fn an_early_submit_is_accepted_at_the_finish_and_not_submitted_again() -> TestResult {
        let (rig, parent) = Rig::fixture("g2-early", Dirt::Clean)?;
        let bench = bench(&rig, parent, "yi/cand-green")?;
        bench.submit()?;
        let line = finish(&bench).ok_or("the engine did not take its own child")?;
        assert!(line.starts_with("plan: accepted"), "{line}");
        assert_eq!(actors(&bench, KIND_CANDIDATE_SUBMITTED)?, ["child-1"]);
        assert_eq!(actors(&bench, KIND_ACCEPTED)?, ["engine"]);
        Ok(())
    }

    // Dies with the `Fail` verdict arm of `finish_child` (finish.rs): a refused candidate
    // leaves its todo running under a child that ended, and only an owner op moves it.
    #[test]
    fn a_red_contract_fails_the_todo_retained() -> TestResult {
        let (rig, parent) = Rig::fixture("g2-red", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-red")?;
        let line = finish(&bench).ok_or("the engine did not take its own child")?;
        assert!(
            line.starts_with("plan: refused \"add rotate\": rotate-listed: "),
            "{line}"
        );
        let TodoState::Failed { cause, .. } = bench.state()? else {
            return Err(format!("expected Failed, got {:?}", bench.state()?).into());
        };
        assert!(
            cause.starts_with("contract refused: rotate-listed: "),
            "{cause}"
        );
        assert_eq!(
            bench.child.disposed.lock().map_err(|_| "poisoned")?[..],
            [Choice::Retained],
            "the refused branch is kept for a retry to read"
        );
        assert!(
            sha(&parent, "yi/cand-red").is_ok(),
            "the candidate is still readable"
        );
        Ok(())
    }

    // Dies with the conflict left running (finish.rs): a retry was illegal and `done` answered
    // "nothing to do", so the todo had no road; the engine fails it and retries it once.
    #[test]
    fn a_merge_conflict_at_the_finish_fails_the_attempt_and_the_engine_retries_it() -> TestResult {
        let (rig, parent) = Rig::fixture("g5-conflict-retry", Dirt::Clean)?;
        let bench = bench(&rig, parent, "yi/cand-conflict")?;
        let line = finish(&bench).ok_or("the engine did not take its own child")?;
        assert!(
            line.contains("conflicts with the parent at rotate.sh"),
            "{line}"
        );
        assert!(line.contains("retried it once"), "{line}");
        let plan = bench.store.read(&bench.plan)?;
        let todo = plan.todo(&TodoLabel::new(LABEL)?).ok_or("todo missing")?;
        assert!(
            matches!(todo.state, TodoState::Running { .. }),
            "{:?}",
            todo.state
        );
        assert_eq!(todo.attempt.get(), 2, "the engine dispatched a new attempt");
        let briefs = bench.child.briefs.lock().map_err(|_| "poisoned")?;
        let told = briefs
            .last()
            .and_then(|delegation| delegation.extra.get("brief_lines"))
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            told.contains("previous attempt failed") && told.contains("rotate.sh"),
            "{told}"
        );
        drop(briefs);
        let again = finish(&bench).ok_or("the engine did not take the second attempt")?;
        assert!(
            !again.contains("retried"),
            "one retry, then the owner's: {again}"
        );
        assert!(matches!(bench.state()?, TodoState::Failed { .. }));
        Ok(())
    }

    // Dies with the `Merge::Conflict` arm of `integrate` (acceptance.rs): return the git error
    // instead and the lane drops through `Drop` with no record naming the branch.
    #[test]
    fn merge_conflict_retains_recoverable_candidate() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-conflict", Dirt::Clean)?;
        let head = sha(&parent, "HEAD")?;
        let bench = bench(&rig, parent.clone(), "yi/cand-conflict")?;
        let refused = bench.submit();
        let Err(PlanOpError::MergeFailed { paths, .. }) = refused else {
            return Err(format!("expected a merge failure, got {refused:?}").into());
        };
        assert_eq!(paths, vec!["rotate.sh".to_owned()]);
        let records = bench.records()?;
        let disposition = records
            .iter()
            .find(|record| record.record.op == KIND_DISPOSITION)
            .ok_or("no disposition record")?;
        let failed = &disposition.args["disposition"]["merge_failed"];
        assert_eq!(failed["branch"], "yi/cand-conflict");
        assert_eq!(failed["parent_base"], head.as_str());
        assert_eq!(
            disposition.args["slot_released"],
            serde_json::json!(false),
            "the worker keeps its checkout until the resolved candidate is submitted again"
        );
        // The way on is a new submit, and `done` says so instead of "a new attempt".
        assert_eq!(
            phase_of(
                &records,
                &bench.plan,
                &TodoLabel::new(LABEL)?,
                AttemptId::FIRST
            ),
            Phase::MergeFailed
        );
        let Err(PlanOpError::PhaseMissing { phase, missing, .. }) = bench.done() else {
            return Err("done after a conflict is refused with the phase".into());
        };
        assert_eq!(phase, "merge_failed");
        assert!(missing.contains("submit"), "{missing}");
        assert_eq!(failed["conflict_provenance"][0]["path"], "rotate.sh");
        assert_eq!(
            failed["conflict_provenance"][0]["base"],
            git(&parent, &["merge-base", "main", "yi/cand-conflict"])?
        );
        assert!(
            sha(&parent, "yi/cand-conflict").is_ok(),
            "the candidate is kept"
        );
        assert_eq!(sha(&parent, "HEAD")?, head, "the parent did not move");
        assert_eq!(held(&bench.pool)?, 0, "the staging slot is released");
        assert_eq!(bench.stage_branches()?, "");
        assert!(
            matches!(bench.state()?, TodoState::Running { .. }),
            "retryable"
        );
        Ok(())
    }

    // Dies with `dispose` preceding `delegate.reap` in `reap_leaving` (acceptance.rs): reap
    // first and a wedged child leaves no record pointing at the only copy of the result.
    #[test]
    fn cleanup_preserves_artifacts_before_releasing_slot() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-order", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-red")?;
        bench.child.fail_reap.store(true, Ordering::SeqCst);
        let wedged = bench.fail(None);
        assert!(
            matches!(wedged, Err(PlanOpError::ReapFailed { .. })),
            "{wedged:?}"
        );
        let kinds = bench.kinds()?;
        assert!(kinds.contains(&KIND_DISPOSITION.to_owned()), "{kinds:?}");
        let records = bench.records()?;
        assert!(
            !records
                .iter()
                .any(|record| record.record.op == "fail" && !record.is_refusal()),
            "the transition waits on the reap: {kinds:?}"
        );
        let disposition = records
            .iter()
            .find(|record| record.record.op == KIND_DISPOSITION)
            .ok_or("no disposition record")?;
        let retained = &disposition.args["disposition"]["retained"];
        assert_eq!(retained["branch"], "yi/cand-red");
        assert!(sha(&parent, "yi/cand-red").is_ok(), "kept resolves");
        assert!(
            matches!(bench.state()?, TodoState::Running { .. }),
            "the slot is what is left"
        );
        bench.child.fail_reap.store(false, Ordering::SeqCst);
        bench.fail(None)?;
        assert!(matches!(bench.state()?, TodoState::Failed { .. }));
        Ok(())
    }

    // Dies with `quiet` in `Lane::candidate` (settle.rs): drop the check and a background
    // writer's bytes are missing from the candidate the checker judges.
    #[test]
    fn writer_is_quiescent_before_snapshot_or_settle() -> TestResult {
        let rig = Rig::new("f0d-quiet")?;
        let pool = rig.pool(2)?;
        let lane = pool.claim("writer", ClaimBase::Main)?;
        let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
        let job = yi_tools::jobs::spawn_job(
            "sleep 1; echo written >> late.txt",
            lane.path(),
            &never,
            None,
        );
        let busy = yi_runtime::lane::settle::quiescence_of_jobs(lane.path());
        assert_eq!(busy.running_commands(), 1);
        let refused = lane.candidate(&busy, None);
        assert!(
            matches!(&refused, Err(LaneError::Busy { running }) if running[0].contains("late.txt")),
            "{refused:?}"
        );
        let other = pool.claim("other", ClaimBase::Main)?;
        let also_refused = other.settle(&busy, None);
        assert!(
            matches!(
                &also_refused,
                Err(Unsettled::Kept(_, LaneError::Busy { .. }))
            ),
            "a busy settle hands the lane back: {also_refused:?}"
        );
        // The lane handed back holds its slot until it is dropped; the count below is this
        // lane's alone.
        drop(also_refused);
        let started = std::time::Instant::now();
        while yi_tools::jobs::registry()
            .report(job)
            .is_some_and(|report| !report.finished)
        {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "the job never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _released = yi_tools::jobs::registry().release(job)?;
        let quiet_now = yi_runtime::lane::settle::quiescence_of_jobs(lane.path());
        assert!(quiet_now.is_quiet());
        let candidate = lane.candidate(&quiet_now, None)?;
        assert_eq!(
            git(
                lane.path(),
                &["show", &format!("{}:late.txt", candidate.commit.as_str())]
            )?,
            "written",
            "the snapshot covers the bytes the command wrote"
        );
        let settled = lane.settle(&quiet_now, None)?;
        assert_eq!(settled.candidate.commit, candidate.commit);
        assert_eq!(held(&pool)?, 0);
        Ok(())
    }

    // Dies with the by-reference candidate in `settle_lane` (settle.rs): take the lane first
    // and a settle the registry refuses drops it, so the slot frees and the pool's next claim
    // resets the tree over the child's uncommitted work.
    #[test]
    fn a_lane_that_cannot_settle_stays_held() -> TestResult {
        let rig = Rig::new("f0d-held")?;
        let pool = rig.pool(2)?;
        let lane = pool.claim("writer", ClaimBase::Main)?;
        let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
        let job = yi_tools::jobs::spawn_job(
            "sleep 1; echo written >> late.txt",
            lane.path(),
            &never,
            None,
        );
        let mut held_lane = Some(lane);
        let refused = yi_runtime::subagent::SubagentHost::settle_lane(&mut held_lane, None, None);
        assert!(
            refused
                .as_ref()
                .is_err_and(|reason| reason.contains("not quiescent")),
            "{refused:?}"
        );
        assert!(held_lane.is_some(), "the lane is still held");
        assert_eq!(held(&pool)?, 1, "the slot is still held");
        let started = std::time::Instant::now();
        while yi_tools::jobs::registry()
            .report(job)
            .is_some_and(|report| !report.finished)
        {
            assert!(started.elapsed() < std::time::Duration::from_secs(20));
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _released = yi_tools::jobs::registry().release(job)?;
        let settled = yi_runtime::subagent::SubagentHost::settle_lane(&mut held_lane, None, None)?
            .ok_or("no lane settled")?;
        assert!(matches!(settled.0, Choice::Retained));
        assert_eq!(
            git(
                &rig.repo,
                &["show", &format!("{}:late.txt", settled.1.commit.as_str())]
            )?,
            "written"
        );
        assert!(held_lane.is_none());
        assert_eq!(held(&pool)?, 0);
        Ok(())
    }

    // Dies with the fast-forward and ref-only publication in `publish` (settle.rs): merge in
    // the parent instead and the accept either refuses on the dirt or commits `scratch.txt`;
    // move the ref alone and `docs/ROTATE.md` is missing from the checkout, shown as a deletion
    // the user's next `git commit -a` would commit over the acceptance.
    #[test]
    fn user_dirty_tree_is_preserved_during_integration() -> TestResult {
        the_users_dirt_survives_the_accept(Dirt::Dirty)
    }

    /// The staged half of the row above, a test of its own so the two halves' hundred-odd git
    /// runs go in parallel: in series they passed the 60 s cutoff on a loaded runner (#721).
    #[test]
    fn user_staged_tree_is_preserved_during_integration() -> TestResult {
        the_users_dirt_survives_the_accept(Dirt::Staged)
    }

    fn the_users_dirt_survives_the_accept(dirt: Dirt) -> TestResult {
        let (rig, parent) = Rig::fixture(&format!("f0d-{}", dirt.arg()), dirt)?;
        let before = parent_view(&parent)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        bench.done()?;
        let records = bench.records()?;
        let accepted = records
            .iter()
            .find(|record| record.record.op == KIND_ACCEPTED)
            .ok_or("no accepted record")?;
        assert_eq!(
            accepted.args["how"], "ref_only",
            "{dirt:?}: the ref moved alone"
        );
        assert_eq!(accepted.record.extra["resolution"], "verified_done");
        let after = parent_view(&parent)?;
        assert_eq!(
            after.0, accepted.args["published"],
            "{dirt:?}: the head moved to the integration"
        );
        assert_ne!(after.0, before.0);
        assert_eq!(
            after.1, before.1,
            "{dirt:?}: the status is the user's dirt, byte for byte"
        );
        assert_eq!(
            after.2, before.2,
            "{dirt:?}: rotate.sh keeps the user's edit"
        );
        assert_eq!(after.3, before.3, "{dirt:?}: scratch.txt is where it was");
        assert_eq!(
            git(&parent, &["log", "--all", "--oneline", "--", "scratch.txt"])?,
            "",
            "the untracked file is in no commit"
        );
        assert!(
            git(&parent, &["show", "HEAD:rotate.sh"])?.contains("list rotate"),
            "the published commit carries the candidate"
        );
        // The path the user never touched follows the ref, in the tree and the index.
        assert_eq!(
            std::fs::read_to_string(parent.join("docs/ROTATE.md"))?,
            git(&parent, &["show", "HEAD:docs/ROTATE.md"])?.to_owned() + "\n",
            "{dirt:?}: the candidate's new file is in the checkout"
        );
        let staged = git(&parent, &["diff", "--cached", "--name-only"])?;
        let unstaged = git(&parent, &["diff", "--name-only"])?;
        match dirt {
            Dirt::Dirty => assert_eq!((staged.as_str(), unstaged.as_str()), ("", "rotate.sh")),
            Dirt::Staged | Dirt::Clean | Dirt::Nested => {
                assert_eq!((staged.as_str(), unstaged.as_str()), ("rotate.sh", ""));
            }
        }
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        assert_eq!(held(&bench.pool)?, 0);
        Ok(())
    }

    // Dies with the `accepted` arm of `replay` (ops.rs): compare the raw op name and the
    // retried `done` is refused `request_id_reused` for an acceptance that succeeded.
    #[test]
    fn a_replayed_done_returns_the_recorded_acceptance() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-replay", Dirt::Clean)?;
        let bench = bench(&rig, parent, "yi/cand-green")?;
        bench.submit()?;
        let first = bench.done_as(Some("req-accept-once"), None)?;
        let records = bench.records()?.len();
        let again = bench.done_as(Some("req-accept-once"), None)?;
        assert_eq!(again.plan.touched, first.plan.touched, "the same answer");
        assert!(
            again
                .notices
                .iter()
                .any(|notice| notice.contains("replayed")),
            "{:?}",
            again.notices
        );
        assert_eq!(
            bench.records()?.len(),
            records,
            "the replay journals nothing"
        );
        assert_eq!(
            bench
                .kinds()?
                .iter()
                .filter(|kind| kind.as_str() == KIND_ACCEPTED)
                .count(),
            1
        );
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        Ok(())
    }

    // Dies with the token comparison in `try_publish` (acceptance.rs): skip it and a product
    // the candidate and integration checks never saw is recorded `VerifiedDone`.
    #[test]
    fn done_naming_an_output_the_integration_never_saw_is_stale() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-stale-output", Dirt::Clean)?;
        let bench = bench(&rig, parent.clone(), "yi/cand-green")?;
        bench.submit()?;
        let head = sha(&parent, "HEAD")?;
        let refused = bench.done_as(None, Some("local://other.sh"));
        assert!(
            matches!(refused, Err(PlanOpError::Stale { .. })),
            "{refused:?}"
        );
        let kinds = bench.kinds()?;
        assert_eq!(
            kinds.last().map(String::as_str),
            Some("verification_stale"),
            "{kinds:?}"
        );
        assert!(!kinds.contains(&KIND_ACCEPTED.to_owned()));
        let todo = bench.store.read(&bench.plan)?;
        let todo = todo.todo(&TodoLabel::new(LABEL)?).ok_or("todo missing")?;
        assert_eq!(todo.refusals, 0, "stale charges nothing");
        assert!(matches!(todo.state, TodoState::Running { .. }));
        assert_eq!(sha(&parent, "HEAD")?, head, "nothing was published");
        // The submitted output is the one the checks saw, and it still accepts.
        bench.done()?;
        assert!(
            matches!(bench.state()?, TodoState::Done { output: Some(output), .. } if output.to_string() == "local://rotate.sh")
        );
        Ok(())
    }

    // Dies with `Purpose::Verification` being its own counter (capacity.rs): charge the
    // checks against the worker share and a parent whose workers hold every worker lane
    // cannot verify the candidate any of them submits.
    #[test]
    fn full_worker_capacity_does_not_deadlock_verification() -> TestResult {
        let (rig, parent) = Rig::fixture("f0d-capacity", Dirt::Clean)?;
        let bench = bench(&rig, parent, "yi/cand-green")?;
        let capacity = bench.engine.capacity();
        let mut workers = Vec::new();
        while let Ok(permit) = capacity.reserve(Purpose::Worker) {
            workers.push(permit);
        }
        assert_eq!(workers.len(), usize::from(capacity.cap(Purpose::Worker)));
        assert!(
            workers.len() < usize::from(bench.pool.slots()),
            "the lane the workers did not get is the reserve"
        );
        bench.submit()?;
        bench.done()?;
        assert!(matches!(bench.state()?, TodoState::Done { .. }));
        assert_eq!(capacity.held(Purpose::Verification), 0);
        assert_eq!(
            usize::from(capacity.held(Purpose::Worker)),
            workers.len(),
            "the acceptance charged nothing to the workers"
        );
        drop(workers);
        assert_eq!(capacity.held(Purpose::Worker), 0);
        assert_eq!(held(&bench.pool)?, 0);
        Ok(())
    }
}

/// Dies with the `Retained` choice `repossess` sets before it settles: discard instead, or drop
/// the lane unsettled, and a revoked child's uncommitted work goes with its checkout.
#[tokio::test]
async fn a_repossessed_worktree_keeps_its_work_on_its_branch() -> TestResult {
    let rig = Rig::new("repossess")?;
    let store = support::memory_store("lanes-repossess");
    let hold = Some("echo unsaved > draft.txt; sleep 30");
    let family = support::family(rig.root.to_path_buf(), rig.repo.clone(), store, hold);
    let mut asked = serde_json::Map::new();
    asked.insert("name".to_owned(), "writer".into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("isolation".to_owned(), "worktree".into());
    family.host.spawn("write a draft".to_owned(), asked)?;
    let tree = family.host.cwd_of("writer").ok_or("no worktree")?;
    for _ in 0..400 {
        if tree.join("draft.txt").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    family.host.revoke("writer", 0, "scope changed")?;
    assert_eq!(family.host.expire().await, ["writer"]);
    let journal = family.journal();
    let Some(yi_types::lease::LeaseRecord::Repossessed(record)) = journal.last() else {
        return Err(format!("no repossession record: {journal:?}").into());
    };
    let kept: Vec<String> = record.kept.iter().map(ToString::to_string).collect();
    let branch = kept
        .iter()
        .find_map(|url| url.strip_prefix("branch://"))
        .ok_or("the record names no branch")?;
    assert_eq!(
        git(&rig.repo, &["show", &format!("{branch}:draft.txt")])?,
        "unsaved"
    );
    Ok(())
}

/// Incident: `yi ask` claims under its session id, then binds that same id, which unlocked
/// and relocked the worktree: two gits before the first request, for a lock already right.
#[test]
fn binding_the_claimed_id_again_leaves_the_lock_alone() -> TestResult {
    let rig = Rig::new("rebind")?;
    let mut lane = rig.pool(1)?.claim("s-same", ClaimBase::Main)?;
    let admin = PathBuf::from(git(lane.path(), &["rev-parse", "--absolute-git-dir"])?);
    let locked = admin.join("locked");
    assert_eq!(std::fs::read_to_string(&locked)?.trim(), "session:s-same");
    std::fs::write(&locked, "marker")?;
    lane.bind_session("s-same")?;
    assert_eq!(std::fs::read_to_string(&locked)?, "marker", "relocked");
    lane.bind_session("s-other")?;
    assert_eq!(std::fs::read_to_string(&locked)?.trim(), "session:s-other");
    Ok(())
}

/// A start asked git for the repository root three times, 10-40 ms each: once for the
/// checkout and again for the lane it claimed, whose root the claim already knew.
#[test]
fn the_repository_root_is_found_once_per_directory() -> TestResult {
    let rig = Rig::new("canonical")?;
    let found = yi_runtime::lane::canonical_repo(&rig.repo).ok_or("not a repo")?;
    let lane = rig.pool(1)?.claim("s-root", ClaimBase::Main)?;
    let moved = rig.root.join("moved.git");
    std::fs::rename(rig.repo.join(".git"), &moved)?;
    for dir in [&rig.repo, &lane.path().to_path_buf()] {
        let again = yi_runtime::lane::canonical_repo(dir);
        assert_eq!(
            again.as_ref(),
            Some(&found),
            "git asked for {}",
            dir.display()
        );
    }
    std::fs::rename(&moved, rig.repo.join(".git"))?;
    drop(lane);
    Ok(())
}

/// Incident: a claim ran `git fetch origin main` whenever the last fetch was a minute old,
/// seconds against a forge; it now starts from the known `origin/main` and fetches beside.
#[test]
fn a_stale_fetch_does_not_hold_the_claim() -> TestResult {
    let rig = Rig::new("fetch-aside")?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let url = format!(
        "http://127.0.0.1:{}/repo.git",
        listener.local_addr()?.port()
    );
    std::thread::spawn(move || {
        let _held = listener.accept();
        std::thread::sleep(std::time::Duration::from_secs(20));
    });
    git(&rig.repo, &["remote", "add", "origin", &url])?;
    git(
        &rig.repo,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    )?;
    let started = std::time::Instant::now();
    let lane = rig.pool(1)?.claim("s-fetch", ClaimBase::Main)?;
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(10), "held {took:?}");
    assert_eq!(
        lane.base().as_deref(),
        Some(git(&rig.repo, &["rev-parse", "HEAD"])?.as_str())
    );
    let local = rig.repo.to_string_lossy().into_owned();
    git(&rig.repo, &["remote", "set-url", "origin", &local])?;
    drop(lane);
    Ok(())
}
