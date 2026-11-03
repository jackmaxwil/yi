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
//!
//! `a_repossessed_worktree_keeps_its_work_on_its_branch` landed with F2b (D215) at the end of
//! this file, against the lease journal on the parent's transcript. The plan journal's own
//! `Disposition::RepossessionPending` is still unwritten: a repossessed worktree todo reaches
//! the plan as a child the host cannot vouch for, blocks on the user, and records its
//! disposition through `fail` or `drop` as any other non-accept exit does. Writing it from
//! the repossession needs a road from the host into the engine's journal, which F3a owns.

use crate::repo::init_repo;
use crate::scratch;
use crate::support;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_runtime::lane::handback::Held;
use yi_runtime::lane::land::{
    LaneHandle, bounded_name, format_lanes, host_of, open_pr_in, owner_repo, parse_jobs, pr_number,
};
use yi_runtime::lane::settle::{
    Prepared, Publish, Published, Quiescence, Unsettled, candidate_of, generation_of, prepare,
    publish,
};
use yi_runtime::lane::{
    BranchName, ClaimBase, DEFAULT_SLOTS, Head, Lane, LaneError, Pool, SlotIndex, SlotView,
    TreeState, head, toolchain,
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
    crate::worktree::git(cwd, args)
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

/// Incident: deleting `~/.yi/lanes` left its worktrees registered, and the next claim was
/// refused for the slot path the pool owns.
#[test]
fn a_claim_rebuilds_a_slot_git_still_registers_without_its_directory() -> TestResult {
    let rig = Rig::new("lanes-gone")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-gone", ClaimBase::Main)?;
    let slot = lane.slot();
    drop(lane);
    let path = pool.dir().join(slot.to_string());
    git(
        &rig.repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "session:s-gone",
            &path.to_string_lossy(),
        ],
    )?;
    std::fs::remove_dir_all(rig.home.join(".yi/lanes"))?;
    let back = pool.claim("s-back", ClaimBase::Main)?;
    assert_eq!(back.slot(), slot);
    assert_eq!(
        git(back.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/s-back"
    );
    let registered = git(&rig.repo, &["worktree", "list", "--porcelain"])?;
    let entries = registered
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count();
    assert_eq!(entries, 2, "the repository and one slot:\n{registered}");
    Ok(())
}

/// Review: the registration of a dead slot took the branch a resume rebuilds, so the claim after a
/// deleted lanes directory died one error later on `already checked out`.
#[test]
fn a_resume_past_a_deleted_lanes_directory_reclaims_a_branch_a_dead_slot_registers() -> TestResult {
    let rig = Rig::new("lanes-gone-resume")?;
    let pool = rig.pool(2)?;
    let idle = pool.claim("s-idle", ClaimBase::Main)?;
    let crashed = pool.claim("s-crash", ClaimBase::Main)?;
    let slot = crashed.slot();
    std::fs::write(crashed.path().join("kept.txt"), "kept\n")?;
    git(crashed.path(), &["add", "kept.txt"])?;
    git(crashed.path(), &["commit", "-qm", "kept"])?;
    drop((idle, crashed));
    orphan(&pool, slot, "s-crash")?;
    assert_ne!(
        slot.get(),
        0,
        "the crashed session sat off the slot a rebuild picks"
    );
    std::fs::remove_dir_all(rig.home.join(".yi/lanes"))?;
    let back = pool.claim("s-crash", ClaimBase::Main)?;
    assert_eq!(
        back.slot().get(),
        0,
        "the rebuild picks the first missing slot"
    );
    assert_eq!(
        git(back.path(), &["symbolic-ref", "--short", "HEAD"])?,
        "yi/s-crash",
        "the resumed session is on the branch its dead slot registered"
    );
    assert_eq!(git(back.path(), &["log", "-1", "--format=%s"])?, "kept");
    let registered = git(&rig.repo, &["worktree", "list", "--porcelain"])?;
    let entries = registered
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count();
    assert_eq!(
        entries, 2,
        "the repository and the rebuilt slot:\n{registered}"
    );
    Ok(())
}

/// Incident: git lists a worktree by realpath, so a symlinked home matched no registration and the
/// sweep cleared nothing.
#[test]
fn a_claim_past_a_deleted_lanes_directory_matches_through_a_symlinked_home() -> TestResult {
    let rig = Rig::new("lanes-gone-link")?;
    let real = rig.root.join("real");
    std::fs::create_dir_all(&real)?;
    let link = rig.root.join("link");
    std::os::unix::fs::symlink(&real, &link)?;
    let pool = Pool::open(&link, &rig.repo, 1)?;
    let lane = pool.claim("s-gone", ClaimBase::Main)?;
    let slot = lane.slot();
    drop(lane);
    let path = pool.dir().join(slot.to_string());
    git(
        &rig.repo,
        &["worktree", "lock", "--reason", "x", &path.to_string_lossy()],
    )?;
    std::fs::remove_dir_all(real.join(".yi/lanes"))?;
    let back = pool.claim("s-back", ClaimBase::Main)?;
    assert_eq!(back.slot(), slot);
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

/// A staging branch an earlier attempt left unmerged is never what the next merge builds on.
#[test]
fn a_left_staging_branch_is_not_the_next_merges_base() -> TestResult {
    let rig = Rig::new("stale-stage")?;
    let pool = rig.pool(3)?;
    let parent = pool.claim("s-parent", ClaimBase::Main)?;
    let generation = generation_of(parent.path(), None)?;
    let base = ClaimBase::Commit(generation.base.as_str().to_owned());
    let stale = pool.claim("stage-sub-1", base.clone())?;
    std::fs::write(stale.path().join("stale.txt"), "stale\n")?;
    git(stale.path(), &["add", "stale.txt"])?;
    git(stale.path(), &["commit", "-qm", "an earlier attempt"])?;
    drop(stale);
    let child = pool.claim("sub-1", base)?;
    std::fs::write(child.path().join("child.txt"), "child\n")?;
    let settled = child.settle(&quiet(), None)?;
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
    let lane = staging.path().ok_or("no staging lane")?;
    assert_eq!(
        git(lane, &["rev-parse", "HEAD^1"])?,
        generation.base.as_str()
    );
    assert!(
        !lane.join("stale.txt").exists(),
        "the earlier attempt is not merged"
    );
    staging.release()?;
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
    std::fs::write(rig.repo.join(".gitignore"), "/target\n")?;
    git(&rig.repo, &["add", "Cargo.lock", ".gitignore"])?;
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
    let built = pool
        .dir()
        .join(slot.to_string())
        .join("target/debug/libyi.rlib");
    std::fs::create_dir_all(built.parent().ok_or("a parent")?)?;
    std::fs::write(&built, "warm\n")?;
    let next = pool.claim("s-next", ClaimBase::Main)?;
    assert_eq!(
        next.slot(),
        slot,
        "ignored build output leaves the slot free"
    );
    assert!(built.is_file(), "and warm");
    Ok(())
}

#[test]
fn a_failing_sync_names_its_command_and_not_the_sandbox_profile() -> TestResult {
    let rig = Rig::new("syncfail")?;
    std::fs::write(rig.repo.join("uv.lock"), "not a lockfile\n")?;
    git(&rig.repo, &["add", "uv.lock"])?;
    git(&rig.repo, &["commit", "-qm", "lock"])?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-syncfail", ClaimBase::Main)?;
    let message = lane
        .sync()
        .err()
        .ok_or("a bad lockfile synced")?
        .to_string();
    assert!(
        message.contains("uv sync --frozen --offline --quiet"),
        "{message}"
    );
    assert!(
        !message.contains("(version 1)") && !message.contains("-D"),
        "{message}"
    );
    assert!(message.len() < 2_000, "{} bytes: {message}", message.len());
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

/// Claims `s-one` on a one-slot pool and leaves an edit to a tracked file and an untracked
/// non-ASCII file in it: the uncommitted work a quit must not hand to the next claim.
fn dirty_lane(rig: &Rig) -> Result<(Pool, Lane, PathBuf, PathBuf), Box<dyn Error>> {
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let (readme, note) = dirty(lane.path())?;
    Ok((pool, lane, readme, note))
}

fn dirty(tree: &Path) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let (readme, note) = (tree.join("README.md"), tree.join("notes-é.txt"));
    std::fs::write(&readme, "edited, not committed\n")?;
    std::fs::write(&note, "brouillon\n")?;
    Ok((readme, note))
}

fn assert_kept_from_the_next_claim(pool: &Pool, readme: &Path, note: &Path) -> TestResult {
    let refused = pool.claim("s-two", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { orphans: 1, .. })),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(readme)?, "edited, not committed\n");
    assert!(note.is_file(), "the untracked file survived the claim");
    Ok(())
}

/// Incident: a clean quit cleared a dirty slot's session, and the next claim ran
/// `checkout -f -B` and `clean -qfd` over the only copy of the work.
#[test]
fn a_clean_quit_keeps_uncommitted_work_from_the_next_claim() -> TestResult {
    let rig = Rig::new("quit-dirty")?;
    let (pool, lane, readme, note) = dirty_lane(&rig)?;
    assert_eq!(lane.release()?.to_string(), "2 uncommitted paths");
    assert_kept_from_the_next_claim(&pool, &readme, &note)?;
    let views = pool.list()?;
    assert!(
        matches!(&views[..], [SlotView::Orphan { holder, .. }] if holder.session == "s-one"),
        "{views:?}"
    );
    let back = pool.claim("s-one", ClaimBase::Main)?;
    assert!(
        back.path().join("notes-é.txt").is_file(),
        "its session resumes it"
    );
    Ok(())
}

/// A submodule at `name` in a lane: its own repository, its gitlink staged and committed, and the
/// `.gitmodules` entry a checkout carries when `modules` asks for one.
fn submodule(lane: &Path, name: &str, modules: bool) -> Result<PathBuf, Box<dyn Error>> {
    let sub = lane.join(name);
    std::fs::create_dir_all(&sub)?;
    git(&sub, &["init", "-q", "-b", "main"])?;
    git(&sub, &["config", "user.email", "t@example.invalid"])?;
    git(&sub, &["config", "user.name", "t"])?;
    std::fs::write(sub.join("file.txt"), "one\n")?;
    git(&sub, &["add", "file.txt"])?;
    git(&sub, &["commit", "-q", "-m", "first"])?;
    if modules {
        std::fs::write(
            lane.join(".gitmodules"),
            format!("[submodule \"{name}\"]\n\tpath = {name}\n\turl = ./nowhere\n"),
        )?;
        git(lane, &["add", ".gitmodules"])?;
    }
    let head = git(&sub, &["rev-parse", "HEAD"])?;
    let link = format!("160000,{},{}", head.trim(), name);
    git(lane, &["update-index", "--add", "--cacheinfo", &link])?;
    git(lane, &["commit", "-q", "-m", "a submodule"])?;
    Ok(sub)
}

/// Host git stays out of a submodule, so `status` cannot see work inside one. Dies with a populated
/// gitlink read as clean: the lane went back to the pool, and the next claim wiped the only copy.
#[test]
fn work_inside_a_submodule_keeps_its_lane() -> TestResult {
    let rig = Rig::new("quit-submodule")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let sub = submodule(lane.path(), "sub", false)?;
    std::fs::write(sub.join("edited.txt"), "only copy\n")?;
    let text = format_lanes(&pool.list()?, None);
    let line = text.lines().next().unwrap_or_default();
    assert!(
        line.contains("1 modified"),
        "the listing counts a gitlink with no .gitmodules: {text}"
    );
    assert_eq!(lane.release()?.to_string(), "1 uncommitted path");
    let refused = pool.claim("s-two", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { orphans: 1, .. })),
        "{refused:?}"
    );
    assert!(
        sub.join("edited.txt").is_file(),
        "the work survived the claim"
    );
    Ok(())
}

/// Dies with every populated submodule counted: each session left an orphan, until `claim` found
/// the pool full in any repository that uses them.
#[test]
fn a_clean_submodule_frees_its_lane_and_an_edit_in_it_keeps_it() -> TestResult {
    let rig = Rig::new("clean-submodule")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let sub = submodule(lane.path(), "sub", false)?;
    std::fs::write(sub.join("file.txt"), "edited\n")?;
    assert_eq!(lane.release()?.to_string(), "1 uncommitted path");
    let back = pool.claim("s-one", ClaimBase::Main)?;
    std::fs::write(back.path().join("sub/file.txt"), "one\n")?;
    assert_eq!(back.release()?.to_string(), "0 uncommitted paths");
    let again = pool.claim("s-one", ClaimBase::Main)?;
    let sub = again.path().join("sub");
    // The incident the parent's own status read names: a config that hides new files reads clean.
    git(&sub, &["config", "status.showUntrackedFiles", "no"])?;
    std::fs::write(sub.join("new.txt"), "work\n")?;
    assert_eq!(again.release()?.to_string(), "1 uncommitted path");
    Ok(())
}

/// Dies with a quoted gitlink path read as no directory: `ls-files` escapes a name beyond ASCII, so
/// a submodule full of edits was skipped and its lane went back to the pool.
#[test]
fn an_edited_submodule_named_beyond_ascii_keeps_its_lane() -> TestResult {
    let rig = Rig::new("quit-submodule-utf8")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let sub = submodule(lane.path(), "sub-é", false)?;
    std::fs::write(sub.join("file.txt"), "edited\n")?;
    assert_eq!(lane.release()?.to_string(), "1 uncommitted path");
    assert!(sub.join("file.txt").is_file(), "the work survived");
    Ok(())
}

#[test]
fn a_dropped_lane_keeps_its_dirty_tree() -> TestResult {
    let rig = Rig::new("drop-dirty")?;
    let (pool, lane, readme, note) = dirty_lane(&rig)?;
    drop(lane);
    assert_kept_from_the_next_claim(&pool, &readme, &note)?;
    let back = pool.claim("s-one", ClaimBase::Main)?;
    assert!(
        back.path().join("notes-é.txt").is_file(),
        "its session resumes it"
    );
    Ok(())
}

/// An older binary's release detached the slot, deleted the merged branch and cleared the
/// session whatever the tree held: a clean release with the work written after is that shape.
#[test]
fn a_slot_an_older_binary_released_dirty_is_never_wiped() -> TestResult {
    let rig = Rig::new("old-release")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let tree = lane.path().to_path_buf();
    drop(lane);
    let (readme, note) = dirty(&tree)?;
    assert_kept_from_the_next_claim(&pool, &readme, &note)?;
    let text = format_lanes(&pool.list()?, None);
    let line = text.lines().next().unwrap_or_default();
    assert!(
        line.starts_with("lane 0: left by no session on (detached) · 1 modified, 1 untracked, 0 ahead, 0 behind · ")
            && line.ends_with(" — reap discards the uncommitted paths"),
        "the refusal's listing names the slot it counted: {text}"
    );
    Ok(())
}

/// Dies with a listing that reads only the parent's status: the submodule's edit is invisible to
/// it, so the row said clean while the claim refused the slot as dirty.
#[test]
fn a_listing_counts_work_inside_a_submodule() -> TestResult {
    let rig = Rig::new("list-submodule")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let sub = submodule(lane.path(), "sub", true)?;
    std::fs::write(sub.join("file.txt"), "edited\n")?;
    let text = format_lanes(&pool.list()?, None);
    let line = text.lines().next().unwrap_or_default();
    assert!(line.contains("1 modified"), "the listing counts it: {text}");
    Ok(())
}

#[test]
fn reap_discards_a_dirty_tree_and_says_how_much() -> TestResult {
    let rig = Rig::new("reap-dirty")?;
    let (pool, lane, readme, note) = dirty_lane(&rig)?;
    let slot = lane.slot();
    lane.release()?;
    let message = pool.reap(slot)?;
    assert!(
        message.contains("2 uncommitted paths discarded"),
        "{message}"
    );
    let next = pool.claim("s-two", ClaimBase::Main)?;
    assert_eq!(next.slot(), slot);
    assert_eq!(git(next.path(), &["status", "--porcelain"])?, "");
    assert_eq!(std::fs::read_to_string(&readme)?, "base\n");
    assert!(!note.exists(), "the reap discarded the untracked file");
    Ok(())
}

/// A status list past the capture cap is counted as a floor, never as the whole: 1,500
/// twenty-byte rows are exactly the cap, and one byte more cuts the list.
#[test]
fn a_reap_past_the_status_cap_says_at_least() -> TestResult {
    let rig = Rig::new("reap-cap")?;
    let pool = rig.pool(1)?;
    for (session, longer) in [("s-at", None), ("s-past", Some(0))] {
        let lane = pool.claim(session, ClaimBase::Main)?;
        for n in 0..1_500 {
            let name = match longer {
                Some(at) if at == n => format!("{n:017}"),
                _ => format!("{n:016}"),
            };
            std::fs::write(lane.path().join(name), "")?;
        }
        let slot = lane.slot();
        lane.release()?;
        let message = pool.reap(slot)?;
        match longer {
            None => assert!(
                message.starts_with("lane 0 freed; 1500 uncommitted paths discarded; branch"),
                "{message}"
            ),
            Some(_) => {
                let floor = message
                    .strip_prefix("lane 0 freed; at least ")
                    .and_then(|rest| rest.split_once(' '))
                    .and_then(|(n, _)| n.parse::<usize>().ok())
                    .ok_or(format!("no floor: {message}"))?;
                assert!((1_490..1_500).contains(&floor), "{message}");
                assert!(
                    message.contains(
                        " uncommitted paths (status read 30000 of 30001 bytes, OUTPUT_CAP 30000) discarded; branch"
                    ),
                    "{message}"
                );
            }
        }
    }
    Ok(())
}

/// A wipe runs `checkout -f` and `clean -qfd` at the lane's top, and git enters no submodule for
/// either. Dies with the reap claiming work it left in place.
#[test]
fn a_reap_says_a_submodules_work_survived_it() -> TestResult {
    let rig = Rig::new("reap-submodule")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let sub = submodule(lane.path(), "sub", false)?;
    std::fs::write(sub.join("file.txt"), "only copy\n")?;
    let slot = lane.slot();
    lane.release()?;
    let message = pool.reap(slot)?;
    assert!(
        message.contains("kept inside a submodule"),
        "the reap says what survived: {message}"
    );
    assert!(
        !message.contains("discarded"),
        "and claims nothing it did not drop: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(sub.join("file.txt"))?,
        "only copy\n"
    );
    let refused = pool.claim("s-two", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { .. })),
        "the slot stays out of the pool while the work does: {refused:?}"
    );
    Ok(())
}

/// A lane a child ran `git init` in keeps that directory: `clean -fd` refuses to remove an
/// untracked repository without a second `-f`. Dies with the survivor named a submodule's.
#[test]
fn a_reap_names_a_nested_repository_as_left_in_place() -> TestResult {
    let rig = Rig::new("reap-nested")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let nested = lane.path().join("nested");
    std::fs::create_dir_all(&nested)?;
    git(&nested, &["init", "-q"])?;
    std::fs::write(nested.join("file.txt"), "only copy\n")?;
    let slot = lane.slot();
    lane.release()?;
    let message = pool.reap(slot)?;
    assert!(
        message.contains("kept in place"),
        "the survivor is named for where it is: {message}"
    );
    assert!(
        !message.contains("inside a submodule"),
        "and not for a submodule nothing measured: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(nested.join("file.txt"))?,
        "only copy\n"
    );
    Ok(())
}

/// A resume finds its own slot first: a lower slot freed meanwhile is not where its work is.
#[test]
fn a_resume_takes_its_own_dirty_slot_over_a_lower_free_one() -> TestResult {
    for commit in [false, true] {
        let rig = Rig::new(if commit {
            "resume-own-commit"
        } else {
            "resume-own"
        })?;
        let pool = rig.pool(2)?;
        let low = pool.claim("s-a", ClaimBase::Main)?;
        let mine = pool.claim("s-b", ClaimBase::Main)?;
        let slot = mine.slot();
        if commit {
            std::fs::write(mine.path().join("done.txt"), "committed\n")?;
            git(mine.path(), &["add", "done.txt"])?;
            git(mine.path(), &["commit", "-qm", "done"])?;
        }
        std::fs::write(mine.path().join("work.txt"), "only copy\n")?;
        mine.release()?;
        low.release()?;
        let back = pool.claim("s-b", ClaimBase::Main)?;
        assert_eq!(back.slot(), slot, "commit={commit}");
        assert!(back.path().join("work.txt").is_file(), "commit={commit}");
        assert_eq!(back.path().join("done.txt").is_file(), commit);
    }
    Ok(())
}

#[test]
fn a_discard_says_what_it_dropped_and_frees_the_slot() -> TestResult {
    let rig = Rig::new("discard-dirty")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    std::fs::write(lane.path().join("draft-é.txt"), "unsaved\n")?;
    assert_eq!(
        lane.discard()?.discarded(),
        "; 1 uncommitted path discarded"
    );
    let next = pool.claim("s-two", ClaimBase::Main)?;
    assert_eq!(git(next.path(), &["status", "--porcelain"])?, "");
    Ok(())
}

/// Incident: `status.showUntrackedFiles=no` hid new files from the dirty read, so a quit
/// over them cleared the session and the next claim cleaned them away.
#[test]
fn a_quit_over_new_files_keeps_them_under_a_quiet_status_config() -> TestResult {
    let rig = Rig::new("quiet-status")?;
    git(&rig.repo, &["config", "status.showUntrackedFiles", "no"])?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let note = lane.path().join("notes-é.txt");
    std::fs::write(&note, "brouillon\n")?;
    lane.release()?;
    let refused = pool.claim("s-two", ClaimBase::Main);
    assert!(
        matches!(refused, Err(LaneError::PoolFull { orphans: 1, .. })),
        "{refused:?}"
    );
    assert!(note.is_file());
    Ok(())
}

/// Incident: the same quiet config read a child's new files as no work, so its candidate
/// was the base commit and the files were cleaned when the slot was handed back.
#[test]
fn a_candidate_commits_new_files_under_a_quiet_status_config() -> TestResult {
    let rig = Rig::new("quiet-candidate")?;
    git(&rig.repo, &["config", "status.showUntrackedFiles", "no"])?;
    let lane = rig.pool(1)?.claim("s-one", ClaimBase::Main)?;
    std::fs::write(lane.path().join("notes-é.txt"), "brouillon\n")?;
    let candidate = lane.candidate(&quiet(), None)?;
    let files = git(
        lane.path(),
        &[
            "-c",
            "core.quotePath=false",
            "show",
            "--name-only",
            "--format=",
            candidate.commit.as_str(),
        ],
    )?;
    assert_eq!(files, "notes-é.txt");
    Ok(())
}

/// Incident: a resume claimed a free slot and `-B` reset its own unmerged branch to `main`.
#[test]
fn a_resumed_session_after_a_clean_quit_keeps_its_commits() -> TestResult {
    let rig = Rig::new("quit-commits")?;
    let pool = rig.pool(2)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    std::fs::write(lane.path().join("work.txt"), "committed\n")?;
    git(lane.path(), &["add", "work.txt"])?;
    git(lane.path(), &["commit", "-qm", "work"])?;
    let tip = git(lane.path(), &["rev-parse", "HEAD"])?;
    lane.release()?;
    let other = pool.claim("s-two", ClaimBase::Main)?;
    let free = other.slot();
    let fresh = pool.claim("s-one", ClaimBase::Main)?;
    assert_ne!(fresh.slot(), free, "the resume made a fresh slot");
    assert_eq!(
        git(fresh.path(), &["rev-parse", "HEAD"])?,
        tip,
        "fresh slot"
    );
    fresh.release()?;
    other.release()?;
    let reused = pool.claim("s-one", ClaimBase::Main)?;
    assert_eq!(reused.slot(), free, "the resume reused a free slot");
    assert_eq!(
        git(reused.path(), &["rev-parse", "HEAD"])?,
        tip,
        "reused slot"
    );
    assert!(reused.path().join("work.txt").is_file());
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
    drop(dirty);
    let clean = pool.claim("s-clean", ClaimBase::Main)?;
    let clean_slot = clean.slot();
    drop(clean);
    // A merged branch goes at release; the crash left it, so it is put back as the crash would.
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
            pinned_ms: None,
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
    lane: Option<yi_runtime::lane::Lane>,
    land_command: Option<Vec<String>>,
) -> (std::sync::Arc<LaneHandle>, Steers) {
    let (events, _keep) = tokio::sync::broadcast::channel(16);
    let (tx, rx) = std::sync::mpsc::channel();
    let steer: std::sync::Arc<yi_runtime::lane::land::SteerFn> =
        std::sync::Arc::new(move |message, _| {
            let _ = tx.send(format!("{message:?}"));
        });
    (LaneHandle::new(lane, None, land_command, events, steer), rx)
}

/// The `/discard` reply names the branch and how many uncommitted paths went with it.
#[test]
fn a_discard_reply_counts_the_paths_it_dropped() -> TestResult {
    let rig = Rig::new("discard-reply")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    dirty(lane.path())?;
    let (handle, _steers) = handle_for(Some(lane), None);
    assert_eq!(
        handle.discard()?,
        "discarded yi/s-one; 2 uncommitted paths discarded; the transcript is kept"
    );
    Ok(())
}

/// A quit over a dirty tree prints where the work stayed and the one call that discards it;
/// a clean quit prints nothing.
#[test]
fn a_quit_over_a_dirty_tree_says_where_the_work_stays() -> TestResult {
    let rig = Rig::new("quit-reply")?;
    let pool = rig.pool(2)?;
    let lane = pool.claim("s-one", ClaimBase::Main)?;
    let path = yi_runtime::lane::land::tilde(lane.path());
    dirty(lane.path())?;
    let (handle, _steers) = handle_for(Some(lane), None);
    assert_eq!(
        handle.release()?,
        Some(format!(
            "lane 0 kept: 2 uncommitted paths in {path}; `yi lanes reap 0` discards them"
        ))
    );
    let (clean, _steers) = handle_for(Some(pool.claim("s-two", ClaimBase::Main)?), None);
    assert_eq!(clean.release()?, None);
    Ok(())
}

/// The status row's lane label is the repo's name alone: the slot number the console's `_yi/workdir`
/// carries was noise beside the session name.
#[test]
fn a_lane_row_names_the_repo_without_the_slot() -> TestResult {
    let rig = Rig::new("row")?;
    let pool = rig.pool(2)?;
    let lane = pool.claim("s-row", ClaimBase::Main)?;
    let path = lane.path().to_path_buf();
    let (handle, _steers) = handle_for(Some(lane), None);
    assert_eq!(handle.row(), Some((path, "repo".to_owned())));
    Ok(())
}

/// Two `/land` calls are one poller; the second is refused at once while the first pushes.
#[test]
fn two_lands_share_one_poller() -> TestResult {
    let rig = Rig::new("poller")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-poll", ClaimBase::Main)?;
    let gate = rig.root.join("gate");
    let pushing = rig.root.join("gate.up");
    std::fs::write(&gate, "")?;
    let held = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        r#": > "$0.up"; while [ -e "$0" ]; do sleep 0.01; done"#.to_owned(),
        gate.to_string_lossy().into_owned(),
    ];
    let (handle, steers) = handle_for(Some(lane), Some(held));
    handle.land("Title")?;
    assert!(until(|| pushing.exists()), "the first push never started");
    let (tx, rx) = std::sync::mpsc::channel();
    let second = std::sync::Arc::clone(&handle);
    std::thread::spawn(move || {
        let _ = tx.send(second.land("Title").map_err(|error| error.to_string()));
    });
    let refused = rx.recv_timeout(std::time::Duration::from_secs(60));
    std::fs::remove_file(&gate)?;
    assert!(
        matches!(&refused, Ok(Err(error)) if error.contains("landing in progress")),
        "{refused:?}, steers {:?}",
        steers.try_iter().collect::<Vec<_>>()
    );
    assert!(
        until(|| handle.land("Title").is_ok()),
        "the poller is free once the first ends"
    );
    handle.release()?;
    Ok(())
}

/// A `/land` that finds no lane gives the poller back: the next one says why, not "in progress".
#[test]
fn a_land_without_a_lane_frees_the_poller() {
    let (handle, _steers) = handle_for(None, None);
    for _ in 0..2 {
        let refused = handle.land("Title");
        assert!(
            matches!(&refused, Err(error) if error.to_string().contains("nothing to land")),
            "{refused:?}"
        );
    }
}

fn until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !ready() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    true
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
    let (handle, steers) = handle_for(Some(lane), None);
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
    use yi_runtime::journal::{Journal, RealFs};
    use yi_runtime::lane::settle::Candidate;
    use yi_runtime::plan::acceptance::{
        Candidate as Proven, KIND_ACCEPTED, KIND_CANDIDATE_SUBMITTED, KIND_DISPOSITION,
        KIND_INTEGRATION_INTENT, KIND_INTEGRATION_PREPARED, KIND_INTEGRATION_STALE,
        KIND_INTEGRATION_VERIFIED, Phase, Quiescence, Verified, phase_of,
    };
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
        let checker = crate::checker::put_checker(
            &store.artifacts(&plan),
            &crate::checker::checker_manifest(CHECK, 10_000, &[], false),
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
            let said = refused.as_ref().err().map(ToString::to_string);
            assert!(
                !said
                    .as_deref()
                    .is_some_and(|text| text.contains("Some(") || text.contains("None")),
                "{said:?}"
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
            None,
        )?;
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
        let plan = yi_runtime::lane::fork::Carry::default();
        let (lane, _rows) =
            yi_runtime::lane::fork::ChildTree::fork(&pool, &rig.repo, "writer", &plan)?;
        let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
        let job = yi_tools::jobs::spawn_job(
            "sleep 1; echo written >> late.txt",
            lane.path(),
            &never,
            None,
            None,
        )?;
        let mut held_lane = Some(lane);
        let refused = yi_runtime::subagent::SubagentHost::settle_lane(&mut held_lane, None, None);
        assert!(
            refused
                .as_ref()
                .is_err_and(|reason| reason.contains("not quiescent")),
            "{refused:?}"
        );
        assert!(held_lane.is_some(), "the lane is still held");
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
}

/// A worktree child `name` of `family`, run on its hold, once its `draft.txt` is on disk.
async fn spawn_writer(family: &support::Family, name: &str) -> TestResult {
    let mut asked = serde_json::Map::new();
    asked.insert("name".to_owned(), name.into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("isolation".to_owned(), "worktree".into());
    family.host.spawn("write a draft".to_owned(), asked)?;
    let tree = family.host.cwd_of(name).ok_or("no worktree")?;
    for _ in 0..400 {
        if tree.join("draft.txt").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    Ok(())
}

/// A child named `writer` in a worktree of the rig's repository, run on `hold`.
async fn writer(rig: &Rig, hold: &'static str) -> Result<support::Family, Box<dyn Error>> {
    let store = support::memory_store("lanes-writer");
    let family = support::family(rig.root.to_path_buf(), rig.repo.clone(), store, Some(hold));
    spawn_writer(&family, "writer").await?;
    Ok(family)
}

/// A `name` hook in the rig's repository that sleeps two seconds the first time git runs it
/// and leaves `hook-ran` behind.
/// A git on PATH ahead of the real one, running `script` before it: the host runs no hook, so a
/// shim is what slows it. nextest gives each test its own process, so it reaches no other test.
fn shim_git(rig: &Rig, script: &str) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH").ok_or("no PATH")?;
    let real = std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .ok_or("no git on PATH")?;
    let dir = rig.root.join("shim");
    std::fs::create_dir_all(&dir)?;
    let body = format!("#!/bin/sh\n{script}\nexec '{}' \"$@\"\n", real.display());
    std::fs::write(dir.join("git"), body)?;
    std::fs::set_permissions(dir.join("git"), std::fs::Permissions::from_mode(0o755))?;
    let joined = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path)))?;
    unsafe { std::env::set_var("PATH", joined) };
    Ok(())
}

/// The first `verb` of the rig takes two seconds and leaves `git-ran`.
fn slow_git(rig: &Rig, verb: &str) -> TestResult {
    let mark = rig.root.join("git-ran");
    shim_git(
        rig,
        &format!(
            "[ \"$1\" = {verb} ] && [ ! -e '{0}' ] && {{ : > '{0}'; sleep 2; }}",
            mark.display()
        ),
    )
}

/// Runs `work` on this current-thread runtime and asks `rlm.status` every 20 ms meanwhile:
/// the executor must keep answering, where a git inside the handler holds it for seconds.
async fn served_while<T: Send + 'static>(
    family: &support::Family,
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> Result<T, Box<dyn Error>> {
    use yi_kernel::client::HostHandlers;
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let job = tokio::spawn(work);
    let (mut polls, mut slowest) = (0, std::time::Duration::ZERO);
    while !job.is_finished() {
        let asked = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let status = registry.dispatch("rlm.status", serde_json::Map::new());
        status.ok_or("rlm.status")?.await?;
        slowest = slowest.max(asked.elapsed());
        polls += 1;
    }
    assert!(
        polls >= 10 && slowest < std::time::Duration::from_millis(200),
        "{polls} status polls ran while the git worked, the slowest after {slowest:?}"
    );
    Ok(job.await?)
}

/// Incident: the hand-back's git ran inside the async handler, so one slow hook held the
/// session's one-thread runtime and every other host call waited behind it.
#[tokio::test]
async fn handing_a_worktree_back_leaves_the_executor_free() -> TestResult {
    use yi_kernel::client::HostHandlers;
    for (method, verb) in [
        ("rlm.merge_worktree", "merge"),
        ("rlm.discard_worktree", "commit"),
        ("rlm.delete_subagent", "commit"),
    ] {
        let rig = Rig::new(verb)?;
        slow_git(&rig, verb)?;
        let family = writer(&rig, "echo unsaved > draft.txt").await?;
        assert!(family.reaches("writer", "finished").await);
        if method == "rlm.delete_subagent" {
            family
                .host
                .mark_disposed("writer", yi_types::plan::op::Choice::Retained)?;
        }
        let mut registry = yi_runtime::HostRegistry::default();
        family.host.register(&mut registry);
        let target = serde_json::json!({"target": "writer"});
        let asked = target.as_object().cloned().unwrap_or_default();
        let handing = registry.dispatch(method, asked).ok_or(method)?;
        let reply = served_while(&family, handing).await??;
        assert!(rig.root.join("git-ran").exists(), "{method}: {reply:?}");
        if method == "rlm.delete_subagent" {
            assert!(reply.contains_key("disposition"), "{reply:?}");
        } else {
            assert_eq!(reply["merged"], method == "rlm.merge_worktree");
        }
    }
    Ok(())
}

/// Incident: a lease expiry settled the repossessed lane's git inside the expire task, so a
/// slow git held the session's one-thread runtime and every host call behind it.
#[tokio::test]
async fn a_lease_expiry_leaves_the_executor_free() -> TestResult {
    let rig = Rig::new("expiry")?;
    slow_git(&rig, "commit")?;
    let family = writer(&rig, "echo unsaved > draft.txt; sleep 30").await?;
    family.host.revoke("writer", 0, "scope changed")?;
    let host = std::sync::Arc::clone(&family.host);
    let expired = served_while(&family, async move { host.expire().await }).await?;
    assert_eq!(expired, ["writer"]);
    assert!(rig.root.join("git-ran").exists());
    Ok(())
}

/// Incident: once the hand-back hopped to the blocking pool, concurrent `rlm.merge_worktree`
/// calls published against one parent checkout at once, and the third lost the generation
/// check twice and returned an error with its lane already released.
#[tokio::test]
async fn gathered_hand_backs_all_merge() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let rig = Rig::new("gathered")?;
    let store = support::memory_store("lanes-gathered");
    let hold = "echo unsaved > draft.txt; : > \"own-$$\"";
    let root = rig.root.to_path_buf();
    let family = support::family_with_slots(root, rig.repo.clone(), store, Some(hold), 6);
    let names = ["first", "second", "third"];
    for name in names {
        spawn_writer(&family, name).await?;
        assert!(family.reaches(name, "finished").await);
    }
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let mut merges = Vec::new();
    for name in names {
        let target = serde_json::json!({"target": name});
        let asked = target.as_object().cloned().unwrap_or_default();
        let merge = registry.dispatch("rlm.merge_worktree", asked);
        merges.push(tokio::spawn(merge.ok_or("rlm.merge_worktree")?));
    }
    for merge in merges {
        let reply = merge.await??;
        assert_eq!(reply["merged"], true, "{reply:?}");
    }
    Ok(())
}

/// Dies with the `Retained` choice `repossess` sets before it settles: discard instead, or drop
/// the lane unsettled, and a revoked child's uncommitted work goes with its checkout.
#[tokio::test]
async fn a_repossessed_worktree_keeps_its_work_on_its_branch() -> TestResult {
    let rig = Rig::new("repossess")?;
    let family = writer(&rig, "echo unsaved > draft.txt; sleep 30").await?;
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

/// The `git` of `args` in `cwd`, success or not, as its exit status and both streams.
fn git_raw(cwd: &Path, args: &[&str]) -> Result<(bool, String), Box<dyn Error>> {
    let out = yi_tools::command("git")
        .current_dir(cwd)
        .args(args)
        .output()?;
    let text = [&out.stdout[..], &out.stderr[..]].concat();
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&text).into_owned(),
    ))
}

/// Dies with a ref-only hand-back that replies `merged` and nothing more: the user's edit to a
/// file the child changed stays on disk without the child's change, so the next commit undoes it.
#[tokio::test]
async fn a_ref_only_hand_back_lists_the_edits_it_kept() -> TestResult {
    let rig = Rig::new("kept-yours")?;
    std::fs::write(rig.repo.join("gone.txt"), "old\n")?;
    git(&rig.repo, &["add", "gone.txt"])?;
    git(&rig.repo, &["commit", "-qm", "add gone"])?;
    let hold =
        "printf 'child\\n' >> README.md; printf 'c\\n' > café.txt; echo x > draft.txt; rm gone.txt";
    let family = writer(&rig, hold).await?;
    assert!(family.reaches("writer", "finished").await);
    for (path, text) in [
        ("README.md", "base\nmine\n"),
        ("café.txt", "mine\n"),
        ("gone.txt", "old\nmine\n"),
        ("elsewhere.txt", "mine\n"),
    ] {
        std::fs::write(rig.repo.join(path), text)?;
    }
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["merged"], true, "{reply:?}");
    assert_eq!(reply["published"], "ref_only", "{reply:?}");
    let mut kept: Vec<&str> = reply
        .get("kept_yours")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("no kept_yours: {reply:?}"))?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    kept.sort_unstable();
    assert_eq!(kept, ["README.md", "café.txt", "gone.txt"], "{reply:?}");
    let next = reply
        .get("next")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("no next sentence: {reply:?}"))?;
    assert!(
        next.contains("git diff HEAD") && next.contains("git show HEAD:"),
        "{next}"
    );
    let (_, diff) = git_raw(&rig.repo, &["diff", "HEAD", "--", "README.md"])?;
    assert!(diff.contains("-child") && diff.contains("+mine"), "{diff}");
    assert_eq!(git(&rig.repo, &["show", "HEAD:README.md"])?, "base\nchild");
    let (_, gone) = git_raw(&rig.repo, &["status", "--porcelain", "--", "gone.txt"])?;
    assert_eq!(gone.trim(), "?? gone.txt", "{gone}");
    assert!(!git_raw(&rig.repo, &["show", "HEAD:gone.txt"])?.0);
    assert!(
        next.contains("missing from HEAD") && next.contains("deleted"),
        "{next}"
    );
    Ok(())
}

/// Dies with a fast-forward reply that carries `kept_yours`: nothing was kept, and a list that
/// is sometimes empty and sometimes absent makes the model test for both.
#[tokio::test]
async fn a_fast_forward_hand_back_keeps_nothing_and_says_so_by_omission() -> TestResult {
    let rig = Rig::new("kept-none")?;
    let family = writer(&rig, "printf 'child\\n' >> README.md; echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["published"], "fast_forward", "{reply:?}");
    assert!(
        !reply.contains_key("kept_yours") && !reply.contains_key("next"),
        "{reply:?}"
    );
    Ok(())
}

/// Dies with a conflict reply that stops at `merged: false`: the child's lane is released, so
/// only the branch it names and a git call that merges or drops it lead anywhere.
#[tokio::test]
async fn a_conflicting_hand_back_names_the_branch_it_kept_and_how_to_use_it() -> TestResult {
    let rig = Rig::new("conflict-next")?;
    let family = writer(&rig, "printf 'child\\n' > README.md; echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    std::fs::write(rig.repo.join("README.md"), "parent\n")?;
    git(&rig.repo, &["commit", "-qam", "parent moved"])?;
    let before = git(&rig.repo, &["rev-parse", "HEAD"])?;
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["merged"], false, "{reply:?}");
    let branch = reply["branch"].as_str().ok_or("no branch")?;
    let next = reply
        .get("next")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("no next sentence: {reply:?}"))?;
    assert!(next.contains(&format!("git merge {branch}")), "{next}");
    assert!(next.contains(&format!("git branch -D {branch}")), "{next}");
    assert_eq!(git(&rig.repo, &["rev-parse", "HEAD"])?, before);
    let (merged, said) = git_raw(&rig.repo, &["merge", "--no-edit", branch])?;
    assert!(!merged && said.contains("CONFLICT"), "{said}");
    let unmerged = git(&rig.repo, &["diff", "--name-only", "--diff-filter=U"])?;
    assert_eq!(
        unmerged,
        reply["conflict_provenance"][0]["path"]
            .as_str()
            .ok_or("no path")?
    );
    git(&rig.repo, &["merge", "--abort"])?;
    git(&rig.repo, &["branch", "-D", branch])?;
    assert!(!git_raw(&rig.repo, &["rev-parse", "--verify", "-q", branch])?.0);
    Ok(())
}

/// The kernel's own `rlm.merge_worktree` request for `writer`, carrying `check`.
fn checked_merge(
    family: &support::Family,
    check: impl Into<serde_json::Value>,
) -> Result<yi_kernel::client::HostFuture, Box<dyn Error>> {
    use yi_kernel::client::HostHandlers;
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let asked = serde_json::json!({"target": "writer", "check": check.into()});
    let asked = asked.as_object().cloned().unwrap_or_default();
    Ok(registry
        .dispatch("rlm.merge_worktree", asked)
        .ok_or("rlm.merge_worktree")?)
}

/// Dies with the hand-back back on `prepare`: a slot and a checkout per merge, at 128 children the
/// wait this exists to remove. A git too old for merge-tree is proved on the road it must take.
#[tokio::test]
async fn a_hand_back_merges_in_the_object_store_with_no_staging_lane() -> TestResult {
    let rig = Rig::new("merge-tree-road")?;
    let version = git(&rig.repo, &["--version"])?;
    let native = yi_runtime::lane::stage::native_road(Some(&version));
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["merged"], true, "{reply:?}");
    assert_eq!(
        git(&rig.repo, &["show", "HEAD:draft.txt"])?,
        "x",
        "{reply:?}"
    );
    assert_eq!(
        rig.home.join(".yi/lanes").exists(),
        !native,
        "this host's git is {version}, so only a road that checks out a tree claims a lane"
    );
    Ok(())
}

/// Dies with the handler dropping `check`: a lane whose work fails it lands here anyway, and the
/// red one must leave this checkout as it was and the work on the branch it names.
#[tokio::test]
async fn a_checked_hand_back_merges_only_a_lane_whose_check_passes() -> TestResult {
    for (label, hold, passes) in [
        ("check-green", "echo x > draft.txt; : > NEW", true),
        ("check-red", "echo x > draft.txt", false),
    ] {
        let rig = Rig::new(label)?;
        let family = writer(&rig, hold).await?;
        assert!(family.reaches("writer", "finished").await);
        let before = git(&rig.repo, &["rev-parse", "HEAD"])?;
        let reply = checked_merge(&family, "test -f NEW")?.await?;
        assert_eq!(reply["merged"], passes, "{label}: {reply:?}");
        if passes {
            assert!(rig.repo.join("NEW").is_file(), "{reply:?}");
            continue;
        }
        assert_eq!(reply["check"]["exit"], 1, "{reply:?}");
        assert_eq!(git(&rig.repo, &["rev-parse", "HEAD"])?, before);
        assert!(!rig.repo.join("draft.txt").exists(), "{reply:?}");
        let branch = reply["branch"].as_str().ok_or("no branch")?;
        assert_eq!(
            git(&rig.repo, &["show", &format!("{branch}:draft.txt")])?,
            "x"
        );
        let next = reply["next"].as_str().ok_or("no next sentence")?;
        assert!(next.contains(&format!("git merge {branch}")), "{next}");
    }
    Ok(())
}

/// Dies with a refused check merging, or the work lost: the check runs on the settled commit, so a
/// refusal leaves the work on the branch the error names and this checkout as it was.
#[tokio::test]
async fn a_check_the_hooks_refuse_leaves_the_work_on_its_branch() -> TestResult {
    let rig = Rig::new("check-refused")?;
    let deny = rig.root.join("deny.sh");
    std::fs::write(
        &deny,
        "#!/bin/sh\nprintf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"no checks here\"}}'\n",
    )?;
    let settings = serde_json::json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
        {"type": "command", "command": format!("sh '{}'", deny.display())}
    ]}]}});
    let home = rig.root.join("hook-home");
    std::fs::create_dir_all(home.join(".claude"))?;
    std::fs::write(
        home.join(".claude/settings.json"),
        serde_json::to_vec(&settings)?,
    )?;
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let managed = rig.root.join("managed");
    family
        .host
        .set_checker(yi_runtime::goal::Checker::from_hooks(Some(
            yi_runtime::claude_hooks::Hooks::load_from(&rig.repo, Some(&home), &managed),
        )));
    let refused = family
        .host
        .merge_worktree("writer", Some("test -f NEW"))
        .err()
        .ok_or("a check the hooks refuse merged anyway")?;
    assert!(refused.contains("PreToolUse hook refused"), "{refused}");
    assert!(
        !rig.repo.join("draft.txt").exists(),
        "a refused check merged"
    );
    let branch = refused
        .split("kept on branch ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or("the error names no branch")?;
    assert_eq!(
        git(&rig.repo, &["show", &format!("{branch}:draft.txt")])?,
        "x"
    );
    Ok(())
}

/// Dies with `check` read as an optional string: a list is no check, and the work lands unrun.
#[tokio::test]
async fn a_check_that_is_not_a_string_is_refused_and_merges_nothing() -> TestResult {
    let rig = Rig::new("check-list")?;
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let refused = checked_merge(&family, vec!["false"])?.await;
    let error = refused.err().ok_or("a list check merged")?;
    assert!(error.contains("check must be a string"), "{error}");
    assert!(!rig.repo.join("draft.txt").exists());
    let reply = checked_merge(&family, "true")?.await?;
    assert_eq!(reply["merged"], true, "{reply:?}");
    Ok(())
}

/// Dies with the red reply naming the branch unconditionally: a child that changed nothing has
/// its branch deleted at release, and `git merge <branch>` would then fail.
#[tokio::test]
async fn a_red_check_on_a_child_that_changed_nothing_names_no_branch() -> TestResult {
    let rig = Rig::new("check-noop")?;
    let family = writer(&rig, "true").await?;
    assert!(family.reaches("writer", "finished").await);
    let reply = checked_merge(&family, "false")?.await?;
    assert_eq!(reply["merged"], false, "{reply:?}");
    let branch = reply["branch"].as_str().ok_or("no branch")?;
    assert!(!git_raw(&rig.repo, &["rev-parse", "--verify", "-q", branch])?.0);
    let next = reply["next"].as_str().ok_or("no next sentence")?;
    assert!(next.contains("no branch was kept"), "{next}");
    assert!(!next.contains(branch), "{next}");
    Ok(())
}

/// Dies with `kill_under` off the discard: a child's server outlives the tree it ran in, and the
/// settle refuses the lane as busy until the 600 s job ends on its own.
#[tokio::test]
async fn a_discard_stops_what_still_runs_in_the_tree_and_says_so() -> TestResult {
    let rig = Rig::new("discard-job")?;
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let listed = git(&rig.repo, &["worktree", "list", "--porcelain"])?;
    let lane = listed
        .split("\n\n")
        .find(|entry| entry.contains("branch refs/heads/yi/sub-"))
        .and_then(|entry| entry.lines().next()?.strip_prefix("worktree "))
        .ok_or("no lane worktree")?;
    let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
    let job = yi_tools::jobs::spawn_job("sleep 600", Path::new(lane), &never, None, None)?;
    let host = std::sync::Arc::clone(&family.host);
    let reply = tokio::task::spawn_blocking(move || host.discard_worktree("writer")).await??;
    let stopped = reply["stopped"].as_str().ok_or("no stopped row")?;
    assert!(stopped.contains("sleep 600"), "{stopped}");
    assert!(
        stopped.contains("a process that left its job keeps running"),
        "{stopped}"
    );
    assert!(
        yi_tools::jobs::registry()
            .report(job)
            .is_some_and(|report| report.finished)
    );
    assert!(!Path::new(lane).join("draft.txt").exists());
    Ok(())
}

/// Dies with `carry` off the allowed keys or the plan: the model's list reaches the copy only here.
#[tokio::test]
async fn a_carry_list_on_the_spawn_copies_an_ignored_file_and_a_non_list_refuses() -> TestResult {
    let rig = Rig::new("carry-kwarg")?;
    std::fs::write(rig.repo.join(".gitignore"), ".env\n")?;
    std::fs::write(rig.repo.join(".env"), "KEY=1\n")?;
    let store = support::memory_store("lanes-carry");
    let family = support::family(rig.root.to_path_buf(), rig.repo.clone(), store, None);
    let mut asked = serde_json::Map::new();
    asked.insert("role".to_owned(), "root".into());
    asked.insert("isolation".to_owned(), "worktree".into());
    let mut bad = asked.clone();
    bad.insert("name".to_owned(), "bad".into());
    bad.insert("carry".to_owned(), ".env".into());
    let refused = family
        .host
        .spawn("look".to_owned(), bad)
        .err()
        .ok_or("a non-list carried")?;
    assert!(refused.contains("carry must be a list"), "{refused}");
    asked.insert("name".to_owned(), "kid".into());
    asked.insert("carry".to_owned(), serde_json::json!([".env"]));
    family.host.spawn("look".to_owned(), asked)?;
    let tree = family.host.cwd_of("kid").ok_or("no worktree")?;
    assert_eq!(
        std::fs::read_to_string(tree.join(".env"))?,
        "KEY=1
"
    );
    family.reaches("kid", "finished").await;
    Ok(())
}

/// A check now runs on the settled commit, so a job still running refuses the hand-back at once as
/// it does with no check: its writes would otherwise land after the commit that was checked.
#[tokio::test]
async fn a_job_still_running_refuses_a_checked_hand_back_as_it_does_any_other() -> TestResult {
    let rig = Rig::new("check-job")?;
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let lane = family.host.cwd_of("writer").ok_or("no worktree")?;
    let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
    let job = yi_tools::jobs::spawn_job("sleep 30", &lane, &never, None, None)?;
    let refused = checked_merge(&family, "true")?.await;
    let refused = refused.err().ok_or("a busy tree was merged")?;
    assert!(refused.contains("not quiescent"), "{refused}");
    assert!(lane.join("draft.txt").exists(), "the work stands");
    yi_tools::jobs::registry().kill(job)?;
    Ok(())
}

/// A red check's tail is the view of its output: whole under the ceiling, past it (or past the
/// capture's own cut) rows name kept over total and the rerun, and that rerun, run where the
/// model would, writes the check's whole output to the file it names.
#[tokio::test]
async fn a_red_checks_tail_names_its_cut() -> TestResult {
    let bytes = |lines: usize| (1..=lines).fold(2, |all, n| all + n.to_string().len() + 1);
    let cases = [
        ("tail-whole", 2_000),
        ("tail-over", 5_000),
        ("tail-captured", 20_000),
    ];
    for (label, lines) in cases {
        let rig = Rig::new(label)?;
        let family = writer(&rig, "echo x > draft.txt").await?;
        assert!(family.reaches("writer", "finished").await);
        let reply = checked_merge(
            &family,
            format!("cat draft.txt; seq 1 {lines}; exit 3").as_str(),
        )?
        .await?;
        let before = git(&rig.repo, &["worktree", "list"])?;
        assert_eq!(reply["check"]["exit"], 3, "{reply:?}");
        let tail = reply["check"]["tail"].as_str().ok_or("no tail")?;
        let (cut, captured) = (
            tail.contains("VIEW_CEILING 16384"),
            tail.contains("OUTPUT_CAP"),
        );
        assert_eq!(
            (cut, captured),
            (lines > 2_000, lines > 5_000),
            "{label}: {tail:.300}"
        );
        if !cut {
            assert_eq!(
                tail.lines().count(),
                lines + 1,
                "whole under the ceiling: {tail:.200}"
            );
            continue;
        }
        assert!(
            tail.contains(&format!("kept 30000 of {} bytes", bytes(lines))) == captured,
            "{label}: {tail:.600}"
        );
        let (_, rest) = tail.split_once("the rest: bash ").ok_or(tail)?;
        let (call, _) = rest.split_once(" > view.log 2>&1").ok_or(rest)?;
        let scratch = Scratch::new("lane-rerun")?;
        let script = format!("{call} > view.log 2>&1");
        let mut bash = yi_tools::command("bash");
        bash.args(["-c", &script]).current_dir(&*scratch);
        let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
        let rerun = yi_tools::run_captured(bash, None, &never, yi_tools::OUTPUT_CAP)?;
        assert_eq!(
            rerun.exit_code,
            Some(3),
            "the check's own exit survives the rerun"
        );
        let log = std::fs::read_to_string(scratch.join("view.log"))?;
        let whole: String = (1..=lines).fold("x\n".to_owned(), |all, n| format!("{all}{n}\n"));
        assert_eq!(log, whole, "the rerun holds the output the view cut");
        assert_eq!(
            git(&rig.repo, &["worktree", "list"])?,
            before,
            "the rerun cleans up"
        );
    }
    Ok(())
}

/// Dies with the check run inside the async handler, where the session's one-thread runtime
/// answers nothing until it ends; past the session deadline it is cut and the reply says so.
#[tokio::test]
async fn a_check_past_the_deadline_is_cut_while_the_executor_serves() -> TestResult {
    let rig = Rig::new("check-deadline")?;
    let family = writer(&rig, "echo x > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let now = std::time::Instant::now();
    let ends = now.checked_add(std::time::Duration::from_secs(2));
    family.host.set_deadline(Some(ends.ok_or("no instant")?));
    let merge = checked_merge(&family, "sleep 30")?;
    let reply = served_while(&family, merge).await??;
    assert!(
        now.elapsed() < std::time::Duration::from_secs(20),
        "{reply:?}"
    );
    assert_eq!(reply["merged"], false, "{reply:?}");
    assert!(reply["check"]["exit"].is_null(), "{reply:?}");
    let tail = reply["check"]["tail"].as_str().ok_or("no tail")?;
    assert!(tail.contains("cut at the session deadline"), "{tail}");
    let branch = reply["branch"].as_str().ok_or("no branch")?;
    assert_eq!(
        git(&rig.repo, &["show", &format!("{branch}:draft.txt")])?,
        "x"
    );
    Ok(())
}

/// #1571: git refusing the merge for an overlapping dirty file starts no merge at all, so the
/// steer carries git's own reason; `merge --abort`'s "no merge to abort" used to replace it.
#[test]
fn a_dirty_lane_refused_the_merge_of_main_names_gits_reason() -> TestResult {
    let rig = Rig::new("mergemain-dirty")?;
    std::fs::write(rig.repo.join("a.txt"), "base\n")?;
    git(&rig.repo, &["add", "a.txt"])?;
    git(&rig.repo, &["commit", "-qm", "base"])?;
    git(
        &rig.repo,
        &["remote", "add", "origin", "file:///nonexistent"],
    )?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-dirty", ClaimBase::Main)?;
    std::fs::write(rig.repo.join("a.txt"), "moved on main\n")?;
    git(&rig.repo, &["commit", "-qam", "main moved"])?;
    let head = git(&rig.repo, &["rev-parse", "HEAD"])?;
    git(
        &rig.repo,
        &["update-ref", "refs/remotes/origin/main", &head],
    )?;
    std::fs::write(lane.path().join("a.txt"), "kept here\n")?;
    let (handle, steers) = handle_for(Some(lane), None);
    handle.land("Land the dirty lane")?;
    let told = steers.recv_timeout(std::time::Duration::from_secs(60))?;
    assert!(
        told.contains("a.txt") && told.contains("would be overwritten"),
        "the steer names the file and git's words: {told}"
    );
    Ok(())
}

/// Every fresh slot's `worktree add` takes `seconds`.
fn sleeping_checkout(rig: &Rig, seconds: u32) -> TestResult {
    shim_git(
        rig,
        &format!("[ \"$1\" = worktree ] && [ \"$2\" = add ] && sleep {seconds}"),
    )
}

fn claim_at_once(pool: &Pool, sessions: usize) -> Vec<Result<Lane, LaneError>> {
    let start = std::sync::Barrier::new(sessions);
    std::thread::scope(|scope| {
        let claims: Vec<_> = (0..sessions)
            .map(|n| {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    pool.claim(&format!("s-par-{n}"), ClaimBase::Main)
                })
            })
            .collect();
        claims
            .into_iter()
            .map(|claim| claim.join().unwrap_or(Err(LaneError::Deadline)))
            .collect()
    })
}

/// Dies with a pool that checks parallel worktree children out one at a time.
#[test]
fn parallel_claims_check_out_side_by_side() -> TestResult {
    let rig = Rig::new("par-claim")?;
    sleeping_checkout(&rig, 1)?;
    let pool = rig.pool(4)?;
    let started = std::time::Instant::now();
    let lanes = claim_at_once(&pool, 4)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let took = started.elapsed();
    let slots: std::collections::BTreeSet<_> = lanes.iter().map(Lane::slot).collect();
    assert_eq!(slots.len(), 4, "four claims hold four slots");
    assert!(
        took < std::time::Duration::from_millis(2_500),
        "four claims over a 1 s checkout hook took {took:?}"
    );
    Ok(())
}

/// #1571: the conflict steer cut its list at twenty files and said nothing about the cut; a
/// model reading it believed it had every conflict.
#[test]
fn a_conflict_steer_names_the_cap_it_was_cut_at() -> TestResult {
    let rig = Rig::new("mergemain-many")?;
    let names: Vec<String> = (0..21).map(|n| format!("f{n}.txt")).collect();
    for name in &names {
        std::fs::write(rig.repo.join(name), "base\n")?;
    }
    let mut added: Vec<&str> = vec!["add"];
    added.extend(names.iter().map(String::as_str));
    git(&rig.repo, &added)?;
    git(&rig.repo, &["commit", "-qm", "base"])?;
    git(
        &rig.repo,
        &["remote", "add", "origin", "file:///nonexistent"],
    )?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-many", ClaimBase::Main)?;
    for name in &names {
        std::fs::write(lane.path().join(name), "the lane moved\n")?;
    }
    git(lane.path(), &["commit", "-qam", "the lane moved"])?;
    for name in &names {
        std::fs::write(rig.repo.join(name), "main moved\n")?;
    }
    git(&rig.repo, &["commit", "-qam", "main moved"])?;
    let head = git(&rig.repo, &["rev-parse", "HEAD"])?;
    git(
        &rig.repo,
        &["update-ref", "refs/remotes/origin/main", &head],
    )?;
    let (handle, steers) = handle_for(Some(lane), None);
    handle.land("Land the many")?;
    let told = steers.recv_timeout(std::time::Duration::from_secs(60))?;
    assert!(
        told.contains(
            "[20 of 21 conflicted files (CONFLICT_FILES_MAX 20); git diff --name-only --diff-filter=U in"
        ),
        "the steer carries the cut's row: {told}"
    );
    Ok(())
}

/// Dies with a sequential repossession in `end`: each settle's commit sleeps three seconds, so
/// two children one after the other take six of the grace.
#[tokio::test]
async fn an_ending_host_repossesses_its_children_side_by_side() -> TestResult {
    let rig = Rig::new("close-two")?;
    let store = support::memory_store("lanes-two");
    let hold = "echo unsaved > draft.txt; sleep 30";
    let (dir, repo) = (rig.root.to_path_buf(), rig.repo.clone());
    let family = support::family_with_slots(dir, repo, store, Some(hold), 3);
    spawn_writer(&family, "first").await?;
    spawn_writer(&family, "second").await?;
    let trees = ["first", "second"].map(|name| family.host.cwd_of(name));
    shim_git(&rig, "case \" $* \" in *\" commit \"*) sleep 3;; esac")?;
    let started = std::time::Instant::now();
    let mut ended = family.host.end().await;
    let took = started.elapsed();
    ended.sort();
    assert_eq!(ended, ["first", "second"]);
    assert!(
        took < std::time::Duration::from_millis(5_500),
        "both came back inside one hook's sleep, not two: {took:?}"
    );
    assert!(
        trees.iter().all(|tree| tree
            .as_deref()
            .is_some_and(|tree| !yi_runtime::lane::fork::is_held(tree))),
        "both forks are released: {trees:?}"
    );
    Ok(())
}

/// #1571: the exits that are not the ACP handle now close their host, so a live worktree
/// child is revoked and its fork released instead of staying held by a session that ended.
#[tokio::test]
async fn an_ending_host_repossesses_a_live_worktree_child_inside_its_grace() -> TestResult {
    let rig = Rig::new("close-slot")?;
    let family = writer(&rig, "echo unsaved > draft.txt; sleep 30").await?;
    let tree = family
        .host
        .cwd_of("writer")
        .ok_or("the child holds no fork")?;
    assert!(
        yi_runtime::lane::fork::is_held(&tree),
        "the child holds its fork"
    );
    assert_eq!(family.host.end().await, ["writer"]);
    assert!(
        !yi_runtime::lane::fork::is_held(&tree),
        "the closed session's child released its fork: {}",
        tree.display()
    );
    Ok(())
}
/// Dies with `judge` or `remove` deleted: a dry run removes nothing, `--fix` takes only the
/// merged clean tree and its `target/`, and each kept tree says why.
#[test]
fn gc_removes_only_the_merged_clean_worktree_and_says_why_it_kept_the_rest() -> TestResult {
    let rig = Rig::new("gc")?;
    let root = rig.root.canonicalize()?;
    let tree = |name: &str| root.join(format!("wt-{name}"));
    std::fs::write(rig.repo.join(".gitignore"), "target/\n")?;
    git(&rig.repo, &["add", ".gitignore"])?;
    git(&rig.repo, &["commit", "-qm", "ignore target"])?;
    for name in ["merged", "dirty", "unmerged", "locked"] {
        let path = tree(name);
        git(
            &rig.repo,
            &["worktree", "add", "-q", "-b", name, &path.to_string_lossy()],
        )?;
        std::fs::write(path.join(format!("{name}.txt")), "work\n")?;
        git(&path, &["add", "."])?;
        git(&path, &["commit", "-qm", name])?;
    }
    for name in ["merged", "dirty", "locked"] {
        git(&rig.repo, &["merge", "-q", "--no-ff", "-m", name, name])?;
    }
    git(&rig.repo, &["branch", "atbase"])?;
    git(
        &rig.repo,
        &[
            "worktree",
            "add",
            "-q",
            &tree("atbase").to_string_lossy(),
            "atbase",
        ],
    )?;
    git(
        &rig.repo,
        &["worktree", "lock", &tree("locked").to_string_lossy()],
    )?;
    std::fs::write(tree("dirty").join("scratch.txt"), "unsaved\n")?;
    std::fs::create_dir_all(tree("merged").join("target"))?;
    std::fs::write(tree("merged").join("target/big"), "x")?;
    let pool = rig.pool(2)?;

    let preview = pool.gc(&rig.repo, true)?.join("\n");
    assert!(preview.contains("would remove"), "{preview}");
    assert!(tree("merged").is_dir(), "a dry run removes nothing");

    let report = pool.gc(&rig.repo, false)?.join("\n");
    assert!(!tree("merged").exists(), "{report}");
    assert!(
        tree("dirty").is_dir() && tree("unmerged").is_dir(),
        "{report}"
    );
    let kept = |name: &str| format!("kept {}", tree(name).display());
    assert!(
        report.contains(&format!("{}: 1 uncommitted path", kept("dirty"))),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "{}: unmerged is not merged into",
            kept("unmerged")
        )),
        "{report}"
    );
    let base = git(&rig.repo, &["rev-parse", "main"])?;
    for (name, why) in [
        ("locked", "locked, a session holds it".to_owned()),
        (
            "atbase",
            format!("atbase is at {base}, no commits of its own"),
        ),
    ] {
        assert!(tree(name).is_dir(), "{report}");
        assert!(
            report.contains(&format!("{}: {why}", kept(name))),
            "{report}"
        );
    }
    assert!(git(&rig.repo, &["rev-parse", "--verify", "-q", "merged"]).is_ok());
    Ok(())
}

/// Dies with the `.held` probe removed from `gc`: a resumed lane is unlocked in git, merged
/// and clean, and only the live session's flock keeps `--fix` from deleting its directory.
#[test]
fn gc_keeps_a_slot_a_live_session_holds_even_when_git_sees_it_merged() -> TestResult {
    let rig = Rig::new("gc-held")?;
    let pool = rig.pool(2)?;
    let lane = pool.claim("s-held", ClaimBase::Main)?;
    std::fs::write(lane.path().join("w.txt"), "work\n")?;
    git(lane.path(), &["add", "."])?;
    git(lane.path(), &["commit", "-qm", "w"])?;
    git(
        &rig.repo,
        &["merge", "-q", "--no-ff", "-m", "m", lane.branch().as_str()],
    )?;
    git(
        &rig.repo,
        &["worktree", "unlock", &lane.path().to_string_lossy()],
    )?;
    let report = pool.gc(&rig.repo, false)?.join("\n");
    assert!(lane.path().is_dir(), "{report}");
    assert!(report.contains("a live session holds the slot"), "{report}");
    Ok(())
}

/// Dies when `judge` refuses a detached HEAD: release leaves merged and unmerged lanes alike
/// detached, and only the commit's ancestry says which tree still holds work.
#[test]
fn gc_judges_a_released_lane_by_its_detached_head() -> TestResult {
    let rig = Rig::new("gc-detached")?;
    let pool = rig.pool(2)?;
    let merged = pool.claim("s-merged", ClaimBase::Main)?;
    let unmerged = pool.claim("s-unmerged", ClaimBase::Main)?;
    for (lane, name) in [(&merged, "merged"), (&unmerged, "unmerged")] {
        std::fs::write(lane.path().join(format!("{name}.txt")), "work\n")?;
        git(lane.path(), &["add", "."])?;
        git(lane.path(), &["commit", "-qm", name])?;
    }
    git(
        &rig.repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merged",
            merged.branch().as_str(),
        ],
    )?;
    let merged_path = merged.path().to_path_buf();
    let unmerged_path = unmerged.path().to_path_buf();
    merged.release()?;
    unmerged.release()?;
    assert!(matches!(head(&merged_path)?, Head::Detached(_)));
    assert!(matches!(head(&unmerged_path)?, Head::Detached(_)));

    let report = pool.gc(&rig.repo, false)?.join("\n");
    assert!(!merged_path.exists(), "{report}");
    assert!(unmerged_path.is_dir(), "{report}");
    assert!(
        report.contains("detached HEAD is not merged into"),
        "{report}"
    );
    Ok(())
}

/// Dies with two claimers handed one fresh slot once the pool lock no longer covers the add.
#[test]
fn an_oversubscribed_pool_never_hands_one_slot_out_twice() -> TestResult {
    let rig = Rig::new("par-full")?;
    let pool = rig.pool(4)?;
    let (mut lanes, mut full) = (Vec::new(), 0);
    for claim in claim_at_once(&pool, 8) {
        match claim {
            Ok(lane) => lanes.push(lane),
            Err(LaneError::PoolFull { slots: 4, .. }) => full += 1,
            Err(error) => return Err(error.into()),
        }
    }
    let slots: std::collections::BTreeSet<_> = lanes.iter().map(Lane::slot).collect();
    assert_eq!((lanes.len(), slots.len(), full), (4, 4, 4));
    Ok(())
}

/// Dies with a claim that signals whatever process reuses a stale warm receipt's pid.
#[test]
fn a_claim_leaves_a_process_that_is_not_the_warmer_alive() -> TestResult {
    let rig = Rig::new("warm-pid")?;
    std::fs::write(rig.repo.join("Cargo.lock"), "# lock\n")?;
    git(&rig.repo, &["add", "Cargo.lock"])?;
    git(&rig.repo, &["commit", "-qm", "lock"])?;
    let pool = rig.pool(1)?;
    let slot = pool.claim("s-first", ClaimBase::Main)?.slot();
    let (cargo, _) = fake_warmer(&rig)?;
    let stranger = yi_tools::command("/bin/sh")
        .arg("-c")
        .arg(r#"/bin/sh "$1" build >/dev/null 2>&1 & echo $!"#)
        .arg("sh")
        .arg(&cargo)
        .output()?;
    let pid = String::from_utf8(stranger.stdout)?.trim().to_owned();
    let path = pool.dir().join(format!("{slot}.json"));
    let mut state: yi_types::lane::SlotState = serde_json::from_slice(&std::fs::read(&path)?)?;
    state.warm = Some(yi_types::lane::WarmReceipt {
        base: String::new(),
        lockfile: String::new(),
        pid: Some(pid.parse()?),
        exit_code: None,
        started_ms: 0,
        extra: Default::default(),
    });
    std::fs::write(&path, serde_json::to_vec(&state)?)?;
    let claimed = pool.claim("s-next", ClaimBase::Main);
    let survived = yi_kernel::bootstrap::process_is_running(pid.parse()?)?;
    let _ = yi_tools::command("/bin/sh")
        .args(["-c", r#"kill -KILL "$1""#, "kill", &pid])
        .status();
    assert_eq!(claimed?.slot(), slot);
    assert!(
        survived,
        "the claim signalled pid {pid}, a `cargo build`, not the warmer"
    );
    Ok(())
}

/// Dies with a `seen.json` that a concurrent sync tore or that lost another sync's hash.
#[test]
fn concurrent_syncs_keep_every_seen_lockfile() -> TestResult {
    let rig = Rig::new("seen")?;
    let pool = rig.pool(8)?;
    let lanes = (0..8)
        .map(|n| pool.claim(&format!("s-seen-{n}"), ClaimBase::Main))
        .collect::<Result<Vec<_>, _>>()?;
    for (n, lane) in lanes.iter().enumerate() {
        std::fs::write(lane.path().join("Cargo.lock"), format!("# lock {n}\n"))?;
    }
    let start = std::sync::Barrier::new(lanes.len());
    std::thread::scope(|scope| {
        for lane in &lanes {
            let start = &start;
            scope.spawn(move || {
                start.wait();
                lane.sync()
            });
        }
    });
    let seen: yi_types::lane::SeenLockfiles =
        serde_json::from_slice(&std::fs::read(pool.dir().join("seen.json"))?)?;
    let hashes: std::collections::BTreeSet<_> = seen.hashes.iter().collect();
    assert_eq!(hashes.len(), 8, "{:?}", seen.hashes);
    Ok(())
}

/// Dies with a sync that runs only the first lockfile's toolchain and leaves `uv.lock` unsynced.
#[test]
fn a_tree_with_two_lockfiles_syncs_both() -> TestResult {
    let rig = Rig::new("two-locks")?;
    std::fs::write(rig.repo.join("Cargo.lock"), "# lock\n")?;
    std::fs::write(rig.repo.join("uv.lock"), "not a lockfile\n")?;
    git(&rig.repo, &["add", "Cargo.lock", "uv.lock"])?;
    git(&rig.repo, &["commit", "-qm", "locks"])?;
    let lane = rig.pool(1)?.claim("s-two", ClaimBase::Main)?;
    let synced = lane.sync();
    let message = synced.err().ok_or("uv.lock was never synced")?.to_string();
    assert!(message.contains("uv sync --frozen --offline"), "{message}");
    Ok(())
}

/// Dies with a session refused outright because its toolchain sync failed.
#[test]
fn a_failed_sync_starts_the_session_with_a_notice() -> TestResult {
    let rig = Rig::new("soft-sync")?;
    std::fs::write(rig.repo.join("uv.lock"), "not a lockfile\n")?;
    git(&rig.repo, &["add", "uv.lock"])?;
    git(&rig.repo, &["commit", "-qm", "lock"])?;
    let pool = rig.pool(1)?;
    let (lane, notice) = yi_runtime::lane::land::claim_root(&pool, "s-soft")?;
    let notice = notice.ok_or("a failed sync said nothing")?;
    assert!(
        notice.starts_with(&format!("lane {}: toolchain sync failed", lane.slot())),
        "{notice}"
    );
    assert!(
        notice.contains("uv sync --frozen --offline --quiet"),
        "{notice}"
    );
    assert!(!notice.contains('\n'), "one line: {notice}");
    Ok(())
}

async fn spawn_reply(rig: &Rig) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let store = support::memory_store("lanes-base");
    let family = support::family(rig.root.to_path_buf(), rig.repo.clone(), store, None);
    let mut asked = serde_json::Map::new();
    asked.insert("name".to_owned(), "kid".into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("isolation".to_owned(), "worktree".into());
    let reply = family.host.spawn("look".to_owned(), asked)?;
    let next = reply["next"].as_str().unwrap_or_default().to_owned();
    let notices = family.notices.lock().map_err(|_| "poisoned")?.clone();
    family.reaches("kid", "finished").await;
    Ok((next, notices))
}

/// Dies with a worktree child cut at HEAD while nothing tells the parent its edits stay behind.
#[tokio::test]
async fn a_worktree_spawn_names_the_edits_its_child_starts_without() -> TestResult {
    let dirty = Rig::new("base-dirty")?;
    std::fs::write(dirty.repo.join("README.md"), "edited\n")?;
    let head = git(&dirty.repo, &["rev-parse", "--short=8", "HEAD"])?;
    let (next, notices) = spawn_reply(&dirty).await?;
    let row = format!("the child starts at HEAD {head}, without 1 uncommitted path");
    assert!(next.starts_with(&row), "{next}");
    assert!(
        notices.iter().any(|notice| notice.contains(&row)),
        "{notices:?}"
    );

    let clean = Rig::new("base-clean")?;
    let (next, notices) = spawn_reply(&clean).await?;
    assert!(!next.contains("uncommitted"), "{next}");
    assert!(
        !notices.iter().any(|notice| notice.contains("uncommitted")),
        "{notices:?}"
    );
    Ok(())
}

/// Dies with a worktree child whose parent's status read failed and so was told nothing at all.
#[tokio::test]
async fn a_worktree_spawn_whose_status_fails_still_names_the_head() -> TestResult {
    let rig = Rig::new("base-unread")?;
    std::fs::write(rig.repo.join(".git/index"), "not an index\n")?;
    let head = git(&rig.repo, &["rev-parse", "--short=8", "HEAD"])?;
    let (next, _) = spawn_reply(&rig).await?;
    let row = format!("the child starts at HEAD {head}; uncommitted paths not counted: git status");
    assert!(next.starts_with(&row), "{next}");
    assert!(next.contains("index"), "git's own cause is dropped: {next}");
    Ok(())
}

fn fake_warmer(rig: &Rig) -> Result<(PathBuf, &'static [&'static str]), Box<dyn Error>> {
    let cargo = rig.root.join("cargo");
    std::fs::write(&cargo, "sleep 30\n")?;
    let warm = yi_runtime::lane::toolchain::TOOLCHAINS
        .iter()
        .find(|toolchain| toolchain.lockfile == "Cargo.lock")
        .ok_or("no Cargo.lock toolchain")?
        .warm;
    Ok((cargo, warm.get(1..).unwrap_or_default()))
}

fn merged_crashed_slot(
    rig: &Rig,
    pool: &Pool,
) -> Result<(PathBuf, PathBuf, yi_types::lane::SlotState), Box<dyn Error>> {
    let lane = pool.claim("s-crashed", ClaimBase::Main)?;
    std::fs::write(lane.path().join("work.txt"), "work\n")?;
    git(lane.path(), &["add", "."])?;
    git(lane.path(), &["commit", "-qm", "work"])?;
    git(
        &rig.repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merged",
            lane.branch().as_str(),
        ],
    )?;
    let path = lane.path().canonicalize()?;
    let slot = lane.slot();
    drop(lane);
    let state_path = pool.dir().join(format!("{slot}.json"));
    let mut state: yi_types::lane::SlotState =
        serde_json::from_slice(&std::fs::read(&state_path)?)?;
    state.session = Some("s-crashed".to_owned());
    std::fs::write(&state_path, serde_json::to_vec(&state)?)?;
    Ok((path, state_path, state))
}

/// Dies when a stale git lock outranks the free `.held` flock: a crashed session leaves no
/// live holder, so its merged lane can be unlocked and removed.
#[test]
fn gc_removes_a_merged_slot_with_a_stale_git_lock() -> TestResult {
    let rig = Rig::new("gc-stale-lock")?;
    std::fs::write(rig.repo.join("Cargo.lock"), "# lock\n")?;
    git(&rig.repo, &["add", "Cargo.lock"])?;
    git(&rig.repo, &["commit", "-qm", "lock"])?;
    let pool = rig.pool(1)?;
    let (path, state_path, mut state) = merged_crashed_slot(&rig, &pool)?;
    let (cargo, warm) = fake_warmer(&rig)?;
    let mut warmer = yi_tools::command("/bin/sh")
        .arg(&cargo)
        .args(warm)
        .current_dir(&path)
        .spawn()?;
    let warmer_pid = warmer.id();
    let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = stopped_tx.send(warmer.wait());
    });
    state.warm = Some(yi_types::lane::WarmReceipt {
        pid: Some(warmer_pid),
        ..Default::default()
    });
    std::fs::write(&state_path, serde_json::to_vec(&state)?)?;
    git(&rig.repo, &["worktree", "lock", &path.to_string_lossy()])?;

    let report = pool.gc(&rig.repo, false)?.join("\n");
    let stopped = stopped_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .is_ok();
    if !stopped {
        yi_tools::command("kill")
            .arg(warmer_pid.to_string())
            .output()?;
    }
    let state: yi_types::lane::SlotState = serde_json::from_slice(&std::fs::read(&state_path)?)?;
    assert!(!path.exists(), "{report}");
    assert!(stopped, "gc removed the tree under its live warmer");
    assert!(state.session.is_none(), "gc left a session on a gone slot");
    assert!(state.warm.is_some_and(|warm| warm.pid.is_none()));
    assert!(
        report.contains(&format!("removed {}", path.display())),
        "{report}"
    );
    Ok(())
}

/// Dies with slot cleanup before `git worktree remove`: a refused removal keeps the tree and
/// the session record that a later claim or reap uses to identify its orphan.
#[test]
fn gc_keeps_slot_state_when_worktree_removal_fails() -> TestResult {
    let rig = Rig::new("gc-remove-fails")?;
    let pool = rig.pool(1)?;
    let (path, state_path, _) = merged_crashed_slot(&rig, &pool)?;

    let shim = Scratch::new("gc-remove-failure-shim")?;
    let path_env = std::env::var_os("PATH").ok_or("no PATH")?;
    let real_git = std::env::split_paths(&path_env)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .ok_or("no git on PATH")?;
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = worktree ] && [ \"$2\" = remove ]; then\n  echo forced-remove-failure >&2\n  exit 1\nfi\nexec '{}' \"$@\"\n",
        real_git.display()
    );
    std::fs::write(shim.join("git"), script)?;
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(shim.join("git"), std::fs::Permissions::from_mode(0o755))?;
    let joined = std::env::join_paths(
        std::iter::once(shim.to_path_buf()).chain(std::env::split_paths(&path_env)),
    )?;
    // nextest isolates this test process, so its failing git reaches no other test.
    unsafe { std::env::set_var("PATH", joined) };

    let report = pool.gc(&rig.repo, false)?.join("\n");
    let state: yi_types::lane::SlotState = serde_json::from_slice(&std::fs::read(&state_path)?)?;
    assert!(path.is_dir(), "{report}");
    assert!(report.contains("forced-remove-failure"), "{report}");
    assert_eq!(state.session.as_deref(), Some("s-crashed"), "{report}");
    Ok(())
}

/// Dies when gc judges against HEAD after both main names vanish: a feature checkout is not
/// evidence that another worktree merged, so the tree stays with the base refusal named.
#[test]
fn gc_without_a_main_ref_keeps_a_tree_its_head_contains() -> TestResult {
    let rig = Rig::new("gc-no-main")?;
    let tree = rig.root.join("tree");
    git(
        &rig.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "merged",
            &tree.to_string_lossy(),
        ],
    )?;
    std::fs::write(tree.join("work.txt"), "work\n")?;
    git(&tree, &["add", "."])?;
    git(&tree, &["commit", "-qm", "work"])?;
    git(
        &rig.repo,
        &["merge", "-q", "--no-ff", "-m", "merged", "merged"],
    )?;
    git(&rig.repo, &["branch", "-m", "feature"])?;

    let report = rig.pool(1)?.gc(&rig.repo, false)?.join("\n");
    assert!(tree.is_dir(), "{report}");
    assert!(report.contains("no main or origin/main"), "{report}");
    Ok(())
}

/// Dies when gc refreshes its base: even a dry run with no linked worktrees must leave
/// the aged FETCH_HEAD untouched instead of dialing origin before it prints a report.
#[test]
fn gc_never_fetches_the_remote_main() -> TestResult {
    let rig = Rig::new("gc-no-fetch")?;
    let remote = rig.root.join("remote");
    git(
        &rig.repo,
        &[
            "clone",
            "-q",
            &rig.repo.to_string_lossy(),
            &remote.to_string_lossy(),
        ],
    )?;
    git(
        &rig.repo,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    )?;
    git(&rig.repo, &["fetch", "-q", "origin"])?;
    let fetch_head = rig.repo.join(".git/FETCH_HEAD");
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture ages FETCH_HEAD so a gc fetch is observable"
    )]
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::open(&fetch_head)?.set_modified(old)?;

    rig.pool(1)?.gc(&rig.repo, true)?;
    assert_eq!(std::fs::metadata(fetch_head)?.modified()?, old);
    Ok(())
}

/// A tree whose hand-back must succeed whatever the repository's hooks and signing would do.
async fn hand_back_commits(rig: &Rig) -> TestResult {
    let family = writer(rig, "echo unsaved > draft.txt").await?;
    assert!(family.reaches("writer", "finished").await);
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["merged"], true, "{reply:?}");
    assert_eq!(
        git(&rig.repo, &["show", "HEAD:draft.txt"])?.trim(),
        "unsaved"
    );
    Ok(())
}

/// Dies with the host commit running the repository's hooks: the hand-back came back `Kept`.
#[tokio::test]
async fn a_failing_pre_commit_hook_does_not_stop_a_hand_back() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new("failing-hook")?;
    let hook = rig.repo.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n")?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    hand_back_commits(&rig).await
}

/// Dies with the host commit signing: a repository set to sign, with no key, refused it.
#[tokio::test]
async fn commit_signing_with_no_key_does_not_stop_a_hand_back() -> TestResult {
    let rig = Rig::new("signing")?;
    git(&rig.repo, &["config", "commit.gpgsign", "true"])?;
    git(&rig.repo, &["config", "gpg.program", "/usr/bin/false"])?;
    hand_back_commits(&rig).await
}

/// Dies with no fallback: git refuses a commit with no identity, and a fresh box has none.
#[tokio::test]
async fn a_repository_with_no_identity_still_hands_back() -> TestResult {
    let rig = Rig::new("no-identity")?;
    git(&rig.repo, &["config", "--unset", "user.email"])?;
    git(&rig.repo, &["config", "--unset", "user.name"])?;
    git(&rig.repo, &["config", "user.useConfigOnly", "true"])?;
    // nextest gives each test its own process, so this empty global config reaches no other.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    hand_back_commits(&rig).await
}

/// Dies with an identity probe keyed on `user.email` alone: with the name unset and `useConfigOnly`
/// on, git refuses the hand-back with `empty ident name … not allowed`.
#[tokio::test]
async fn a_repository_with_only_an_email_still_hands_back() -> TestResult {
    let rig = Rig::new("email-only-identity")?;
    git(&rig.repo, &["config", "--unset", "user.name"])?;
    git(&rig.repo, &["config", "user.email", "t@example.invalid"])?;
    git(&rig.repo, &["config", "user.useConfigOnly", "true"])?;
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    hand_back_commits(&rig).await
}

/// Dies with `git add -A` over everything: a build's output landed in the candidate. Source under
/// a directory that merely shares the name still lands, and so does a tracked build file.
#[tokio::test]
async fn untracked_build_output_stays_out_and_source_named_alike_goes_in() -> TestResult {
    let rig = Rig::new("build-output")?;
    std::fs::create_dir_all(rig.repo.join("build"))?;
    std::fs::write(rig.repo.join("build/keep.txt"), "old\n")?;
    git(&rig.repo, &["add", "build"])?;
    git(&rig.repo, &["commit", "-qm", "track a build file"])?;
    let hold = "mkdir -p target/debug src/build node_modules/x .venv/lib dist; \
        echo o > target/debug/x; echo o > node_modules/x/y; echo o > .venv/lib/z; echo o > dist/a; \
        echo o > build/out; echo s > src/build/gen.rs; echo new > build/keep.txt";
    let family = writer(&rig, hold).await?;
    assert!(family.reaches("writer", "finished").await);
    family.host.merge_worktree("writer", None)?;
    let names = git(&rig.repo, &["ls-tree", "-r", "--name-only", "HEAD"])?;
    assert!(
        names.lines().any(|name| name == "src/build/gen.rs"),
        "{names}"
    );
    assert!(!names.contains("target/"), "{names}");
    assert!(!names.contains("node_modules/"), "{names}");
    assert!(!names.contains(".venv/"), "{names}");
    assert!(!names.contains("dist/"), "{names}");
    assert!(!names.contains("build/out"), "{names}");
    assert_eq!(
        git(&rig.repo, &["show", "HEAD:build/keep.txt"])?.trim(),
        "new"
    );
    Ok(())
}

/// Dies with no guard on `HEAD`: the admin `HEAD` is contained-writable, and a planted one walked
/// the host's commit onto trunk.
#[test]
fn a_planted_head_cannot_walk_the_host_commit_onto_trunk() -> TestResult {
    let rig = Rig::new("planted-head")?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-planted", ClaimBase::Main)?;
    let trunk_before = git(&rig.repo, &["rev-parse", "main"])?;
    std::fs::write(lane.path().join("draft.txt"), "x\n")?;
    let admin = git(lane.path(), &["rev-parse", "--absolute-git-dir"])?;
    std::fs::write(
        Path::new(admin.trim()).join("HEAD"),
        "ref: refs/heads/main\n",
    )?;
    let branch = lane.branch().clone();
    let refused = candidate_of(lane.path(), "s-planted", &branch, None, &quiet(), None);
    assert!(
        matches!(refused, Err(LaneError::ForeignHead { .. })),
        "{refused:?}"
    );
    let kept = git(&rig.repo, &["branch", "yi/kept-by-a-gone-lane"]);
    assert!(kept.is_ok(), "{kept:?}");
    std::fs::write(
        Path::new(admin.trim()).join("HEAD"),
        "ref: refs/heads/yi/kept-by-a-gone-lane\n",
    )?;
    let refused = candidate_of(lane.path(), "s-planted", &branch, None, &quiet(), None);
    assert!(
        matches!(refused, Err(LaneError::ForeignHead { .. })),
        "{refused:?}"
    );
    assert_eq!(git(&rig.repo, &["rev-parse", "main"])?, trunk_before);
    Ok(())
}

/// Dies with a relative `core.hooksPath` being honoured: it resolves inside the child's tree, so
/// the child wrote what the host then ran.
#[test]
fn a_hook_in_the_childs_own_tree_never_runs_on_the_host() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new("relative-hooks")?;
    git(&rig.repo, &["config", "core.hooksPath", ".husky"])?;
    let pool = rig.pool(1)?;
    let lane = pool.claim("s-husky", ClaimBase::Main)?;
    let marker = rig.root.join("ran-on-host");
    let hooks = lane.path().join(".husky");
    std::fs::create_dir_all(&hooks)?;
    let hook = hooks.join("pre-commit");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display()))?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let branch = lane.branch().clone();
    candidate_of(lane.path(), "s-husky", &branch, None, &quiet(), None)?;
    assert!(!marker.exists(), "the child's hook ran on the host");
    Ok(())
}

/// Dies with the lane code assuming `main`: a `master` repository's lane based on its own `HEAD`.
#[test]
fn a_claim_bases_on_the_trunk_origin_names() -> TestResult {
    let rig = Rig::new("master-trunk")?;
    let origin = rig.root.join("origin.git");
    let origin_text = origin.to_string_lossy();
    git(
        &rig.root,
        &[
            "clone",
            "-q",
            "--bare",
            &rig.repo.to_string_lossy(),
            &origin_text,
        ],
    )?;
    git(&origin, &["branch", "-q", "-m", "main", "master"])?;
    git(&origin, &["symbolic-ref", "HEAD", "refs/heads/master"])?;
    git(&rig.repo, &["remote", "add", "origin", &origin_text])?;
    git(&rig.repo, &["fetch", "-q", "origin"])?;
    git(&rig.repo, &["remote", "set-head", "origin", "master"])?;
    git(
        &rig.repo,
        &["commit", "-q", "--allow-empty", "-m", "local only"],
    )?;
    let lane = rig.pool(1)?.claim("s-master", ClaimBase::Main)?;
    assert_eq!(
        git(lane.path(), &["rev-parse", "HEAD"])?,
        git(&rig.repo, &["rev-parse", "origin/master"])?
    );
    Ok(())
}

/// Dies with the runner cutting a long `status -z`: the user's file sat in the cut middle, read as
/// clean, and the ref-only restore wrote the child's bytes over it.
#[tokio::test]
async fn a_dirty_file_deep_in_a_long_status_keeps_its_bytes() -> TestResult {
    let rig = Rig::new("long-status")?;
    let hold = "mkdir -p m; printf 'child\\n' >> README.md; printf 'child\\n' > m/new.txt";
    let family = writer(&rig, hold).await?;
    assert!(family.reaches("writer", "finished").await);
    std::fs::create_dir_all(rig.repo.join("a"))?;
    std::fs::create_dir_all(rig.repo.join("m"))?;
    std::fs::create_dir_all(rig.repo.join("z"))?;
    for n in 0..1500 {
        std::fs::write(rig.repo.join(format!("a/untracked-{n:05}.txt")), "x")?;
        std::fs::write(rig.repo.join(format!("z/untracked-{n:05}.txt")), "x")?;
    }
    std::fs::write(rig.repo.join("README.md"), "base\nmine\n")?;
    std::fs::write(rig.repo.join("m/new.txt"), "mine\n")?;
    let reply = family.host.merge_worktree("writer", None)?;
    assert_eq!(reply["published"], "ref_only", "{reply:?}");
    assert_eq!(
        std::fs::read_to_string(rig.repo.join("m/new.txt"))?,
        "mine\n"
    );
    Ok(())
}
