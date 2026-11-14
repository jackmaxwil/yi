//! `Tree` from a kernel's side: requests over names, on real repositories. The kernel is a contained
//! process, so a bad name or a bad list is a refusal, and a verb never takes a path.
use crate::git_lane::git;
use crate::scratch::Scratch;
use crate::support;

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use yi_kernel::client::HostHandlers;

type TestResult = Result<(), Box<dyn Error>>;

struct Rig {
    _root: Scratch,
    repo: PathBuf,
    family: support::Family,
    registry: yi_runtime::HostRegistry,
}

fn rig(tag: &str) -> Result<Rig, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-tree-{tag}"))?;
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join(".gitignore"), "target/\n.env\n")?;
    for n in 0..3 {
        std::fs::write(repo.join(format!("f{n}.txt")), format!("{n}\n"))?;
    }
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "first"])?;
    let store = support::memory_store("tree-verbs");
    let family = support::family(root.to_path_buf(), repo.clone(), store, Some("true"));
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    Ok(Rig {
        _root: root,
        repo,
        family,
        registry,
    })
}

impl Rig {
    async fn ask(&self, method: &str, payload: Value) -> Result<Map<String, Value>, String> {
        let asked = payload.as_object().cloned().unwrap_or_default();
        let call = self
            .registry
            .dispatch(method, asked)
            .ok_or_else(|| format!("no {method}"))?;
        call.await
    }

    fn worktrees(&self) -> Result<Vec<PathBuf>, Box<dyn Error>> {
        let listed = git(&self.repo, &["worktree", "list", "--porcelain"])?;
        Ok((listed.split("\n\n"))
            .filter_map(|entry| entry.lines().next()?.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect())
    }

    async fn fork(&self, name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let before = self.worktrees()?;
        self.ask("tree.fork", json!({"name": name})).await?;
        let added = self
            .worktrees()?
            .into_iter()
            .find(|tree| !before.contains(tree));
        Ok(added.ok_or("no new worktree")?)
    }
}

/// `merge-tree` on two snapshots needs git 2.43 and one forge runner has 2.34, so a git that cannot
/// fold is still made to answer: each of these tests asserts the refusal instead of returning green.
fn foldable(rig: &Rig) -> Result<bool, Box<dyn Error>> {
    Ok(yi_runtime::lane::stage::folds(&git(
        &rig.repo,
        &["--version"],
    )?))
}

/// What the shim does with `git merge-tree`: answer a successful trial, or fail it as an old git
/// would (exit 129).
#[derive(Clone, Copy)]
enum MergeTree {
    Succeeds,
    Fails,
}

/// The host git reads as another version, and answers `merge-tree` the way `merge` says.
fn read_version_as(rig: &Rig, version: &str, merge: MergeTree) -> Result<(), Box<dyn Error>> {
    let answer = match merge {
        MergeTree::Succeeds => format!(
            "for candidate do :; done; exec '{}' rev-parse \"${{candidate}}^{{tree}}\"",
            support::real_git()?.display()
        ),
        MergeTree::Fails => "exit 129".to_owned(),
    };
    let script = format!(
        "[ \"$1\" = --version ] && {{ echo '{version}'; exit 0; }}\ncase \"$*\" in *merge-tree*) {answer};; esac"
    );
    support::shim_git(rig.repo.parent().ok_or("no scratch root")?, &script)
}

fn read(path: &Path) -> Result<String, Box<dyn Error>> {
    Ok(std::fs::read_to_string(path)?)
}

/// Dies with a fork cut at `HEAD`: a child sent to finish an edit would start without it, the failure
/// the isolation it replaces cannot avoid.
#[tokio::test]
async fn a_fork_has_the_dirt_its_parent_keeps_and_shares_nothing_after() -> TestResult {
    let rig = rig("dirt")?;
    std::fs::write(rig.repo.join("f0.txt"), "edited\n")?;
    std::fs::write(rig.repo.join("new.txt"), "new\n")?;
    std::fs::write(rig.repo.join(".env"), "KEY=1\n")?;
    let at = rig.fork("fix").await?;
    assert_eq!(read(&at.join("f0.txt"))?, "edited\n");
    assert_eq!(read(&at.join("new.txt"))?, "new\n");
    assert!(!at.join(".env").exists(), "ignored files take carry=");
    std::fs::write(at.join("f1.txt"), "the fork's\n")?;
    assert_eq!(read(&rig.repo.join("f1.txt"))?, "1\n");
    Ok(())
}

#[tokio::test]
async fn a_fork_at_a_tree_id_is_those_files_with_no_dirt() -> TestResult {
    let rig = rig("at")?;
    std::fs::write(rig.repo.join("f0.txt"), "then\n")?;
    let head = rig.ask("tree.head", json!({"tree": "self"})).await?;
    let id = head["tree"].as_str().ok_or("no id")?;
    std::fs::write(rig.repo.join("f0.txt"), "now\n")?;
    rig.ask("tree.fork", json!({"name": "past", "at": id}))
        .await?;
    let again = rig.ask("tree.head", json!({"tree": "past"})).await?;
    let tree_of = |id: &str| git(&rig.repo, &["rev-parse", &format!("{id}^{{tree}}")]);
    assert_eq!(
        tree_of(id)?,
        tree_of(again["tree"].as_str().ok_or("no id")?)?
    );
    Ok(())
}

#[tokio::test]
async fn a_name_is_checked_and_a_second_fork_of_one_name_refuses() -> TestResult {
    let rig = rig("names")?;
    for bad in ["../up", "a/b", "", "self"] {
        let refused = rig.ask("tree.fork", json!({"name": bad})).await;
        assert!(refused.is_err(), "{bad:?} was taken");
    }
    let long = "x".repeat(65);
    assert!(rig.ask("tree.fork", json!({"name": long})).await.is_err());
    rig.ask("tree.fork", json!({"name": "x".repeat(64)}))
        .await?;
    rig.ask("tree.fork", json!({"name": "one"})).await?;
    assert!(rig.ask("tree.fork", json!({"name": "one"})).await.is_err());
    let many: Vec<String> = (0..257).map(|n| n.to_string()).collect();
    let refused = rig
        .ask("tree.merge", json!({"tree": "self", "others": many}))
        .await;
    assert!(
        refused
            .err()
            .is_some_and(|why| why.contains("the cap is 256"))
    );
    Ok(())
}

#[tokio::test]
async fn a_diff_names_eight_files_cuts_at_nine_and_says_how_to_read_the_rest() -> TestResult {
    let rig = rig("rows")?;
    let at = rig.fork("wide").await?;
    for n in 0..8 {
        std::fs::write(at.join(format!("added{n}.txt")), "x\n")?;
    }
    let reply = rig.ask("tree.diff", json!({"tree": "wide"})).await?;
    let files = reply["files"].as_array().ok_or("no files")?;
    assert_eq!(files.len(), 8, "{reply:?}");
    assert_eq!(reply["summary"], "8 files differ");
    std::fs::write(at.join("added8.txt"), "x\n")?;
    let reply = rig.ask("tree.diff", json!({"tree": "wide"})).await?;
    let files = reply["files"].as_array().ok_or("no files")?;
    assert_eq!(files.len(), 9, "eight rows and the notice: {reply:?}");
    let notice = files[8].as_str().ok_or("no notice")?;
    assert!(notice.contains("8 of 9 files (tree.rows 8)"), "{notice}");
    assert!(
        notice.contains("read(\"tree://wide/\", offset="),
        "{notice}"
    );
    Ok(())
}

#[tokio::test]
async fn would_conflict_names_the_path_and_is_empty_when_nothing_would() -> TestResult {
    let rig = rig("radar")?;
    let at = rig.fork("radar").await?;
    std::fs::write(at.join("f0.txt"), "theirs\n")?;
    if !foldable(&rig)? {
        let seen = rig.ask("tree.diff", json!({"tree": "radar"})).await?;
        assert_eq!(
            seen["would_conflict"],
            Value::Null,
            "a trial that could not run is not no conflict: {seen:?}"
        );
        return Ok(());
    }
    let clear = rig.ask("tree.diff", json!({"tree": "radar"})).await?;
    assert_eq!(clear["would_conflict"], json!([]), "{clear:?}");
    std::fs::write(rig.repo.join("f0.txt"), "ours\n")?;
    git(&rig.repo, &["commit", "-q", "-am", "ours"])?;
    let hit = rig.ask("tree.diff", json!({"tree": "radar"})).await?;
    assert_eq!(hit["would_conflict"], json!(["f0.txt"]), "{hit:?}");
    Ok(())
}

/// Dies with a fold or a trial that runs on a git too old for it: both name the floor they need, and
/// a trial that could not run reads unknown, never "no conflict".
#[tokio::test]
async fn an_old_git_is_refused_by_name_and_its_trial_is_not_read_as_no_conflict() -> TestResult {
    let rig = rig("oldgit")?;
    read_version_as(&rig, "git version 2.34.1", MergeTree::Fails)?;
    let at = rig.fork("oldgit").await?;
    std::fs::write(at.join("f0.txt"), "theirs\n")?;
    let seen = rig.ask("tree.diff", json!({"tree": "oldgit"})).await?;
    assert_eq!(seen["would_conflict"], Value::Null, "{seen:?}");
    let why = rig
        .ask("tree.merge", json!({"tree": "self", "others": ["oldgit"]}))
        .await
        .err()
        .ok_or("a fold ran on git 2.34")?;
    assert!(why.contains("needs git 2.43"), "{why}");
    assert!(why.contains("2.34.1"), "{why}");
    Ok(())
}

/// Dies with a fold that commits or that moves on conflict: the tree a conflict stopped must stand
/// exactly as it was, and a clean one leaves the person's branch where it is.
#[tokio::test]
async fn a_merge_folds_what_it_can_names_what_it_cannot_and_commits_nothing() -> TestResult {
    let rig = rig("merge")?;
    let (clean, clash) = (rig.fork("clean").await?, rig.fork("clash").await?);
    std::fs::write(clean.join("f1.txt"), "clean's\n")?;
    std::fs::write(clash.join("f0.txt"), "clash's\n")?;
    if !foldable(&rig)? {
        let why = rig
            .ask("tree.merge", json!({"tree": "self", "others": ["clean"]}))
            .await
            .err()
            .ok_or("a fold ran on a git that cannot fold")?;
        assert!(why.contains("needs git 2.43"), "{why}");
        return Ok(());
    }
    std::fs::write(rig.repo.join("f0.txt"), "ours\n")?;
    let head = git(&rig.repo, &["rev-parse", "HEAD"])?;
    let reply = rig
        .ask(
            "tree.merge",
            json!({"tree": "self", "others": ["clean", "clash"]}),
        )
        .await?;
    assert_eq!(reply["summary"], "merged 1 of 2", "{reply:?}");
    assert_eq!(reply["merged"], json!(["clean"]));
    assert_eq!(reply["conflicts"], json!(["clash: f0.txt"]));
    assert_eq!(read(&rig.repo.join("f1.txt"))?, "clean's\n");
    assert_eq!(
        read(&rig.repo.join("f0.txt"))?,
        "ours\n",
        "a conflict moved nothing"
    );
    assert_eq!(git(&rig.repo, &["rev-parse", "HEAD"])?, head);
    assert!(git(&rig.repo, &["status", "--porcelain"])?.contains("f1.txt"));
    assert_eq!(
        git(&rig.repo, &["diff", "--cached", "--name-only"])?,
        "",
        "nothing staged"
    );
    Ok(())
}

#[tokio::test]
async fn a_rewind_puts_files_back_and_keeps_a_commit_where_it_was() -> TestResult {
    let rig = rig("rewind")?;
    let at = rig.fork("back").await?;
    std::fs::write(at.join("f0.txt"), "first\n")?;
    let then = rig.ask("tree.head", json!({"tree": "back"})).await?;
    let id = then["tree"].as_str().ok_or("no id")?;
    std::fs::write(at.join("f0.txt"), "second\n")?;
    std::fs::write(at.join("f1.txt"), "second\n")?;
    std::fs::write(at.join("later.txt"), "new since\n")?;
    std::fs::write(at.join("kept.txt"), "new since\n")?;
    rig.ask(
        "tree.rewind",
        json!({"tree": "back", "to": id, "paths": ["f1.txt"]}),
    )
    .await?;
    assert_eq!(read(&at.join("f1.txt"))?, "1\n");
    assert_eq!(read(&at.join("f0.txt"))?, "second\n", "paths limit it");
    rig.ask(
        "tree.rewind",
        json!({"tree": "back", "to": id, "paths": ["later.txt"]}),
    )
    .await?;
    assert!(!at.join("later.txt").exists(), "a named new file goes");
    assert!(at.join("kept.txt").exists(), "an unnamed one stays");
    let up = rig
        .ask(
            "tree.rewind",
            json!({"tree": "back", "to": id, "paths": ["../x"]}),
        )
        .await;
    assert!(
        up.err()
            .is_some_and(|why| why.contains("not a path inside"))
    );
    rig.ask("tree.rewind", json!({"tree": "back", "to": id}))
        .await?;
    assert_eq!(read(&at.join("f0.txt"))?, "first\n");
    assert_eq!(
        git(&at, &["diff", "--cached", "--name-only"])?,
        "",
        "nothing staged"
    );
    assert!(
        rig.ask(
            "tree.rewind",
            json!({"tree": "back", "to": "HEAD; rm -rf /"})
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn a_check_answers_green_red_and_reads_its_run_once() -> TestResult {
    let rig = rig("check")?;
    let at = rig.fork("c").await?;
    std::fs::write(at.join("f0.txt"), "edited\n")?;
    let green = rig
        .ask(
            "tree.check",
            json!({"tree": "c", "cmd": "grep -q edited f0.txt", "wait_s": 30}),
        )
        .await?;
    assert_eq!(green["state"], "green", "{green:?}");
    let red = rig
        .ask(
            "tree.check",
            json!({"tree": "c", "cmd": "grep -q absent f0.txt", "wait_s": 30}),
        )
        .await?;
    assert_eq!(red["state"], "red", "{red:?}");
    assert_eq!(red["exit_code"], 1);
    let running = rig
        .ask(
            "tree.check",
            json!({"tree": "c", "cmd": "sleep 1", "wait_s": 0}),
        )
        .await?;
    assert_eq!(running["state"], "running", "{running:?}");
    let read = rig
        .ask(
            "tree.check",
            json!({"tree": "c", "cmd": "sleep 1", "wait_s": 30}),
        )
        .await?;
    assert_eq!(read["state"], "green", "{read:?}");
    Ok(())
}

#[tokio::test]
async fn a_discard_removes_a_free_tree_and_a_childs_is_refused() -> TestResult {
    let rig = rig("discard")?;
    let at = rig.fork("gone").await?;
    rig.ask("tree.discard", json!({"tree": "gone"})).await?;
    assert!(!at.exists());
    assert!(rig.ask("tree.head", json!({"tree": "gone"})).await.is_err());
    assert!(
        rig.ask("tree.discard", json!({"tree": "gone"}))
            .await
            .is_err()
    );
    let kept = rig.fork("given").await?;
    let mut asked = Map::new();
    asked.insert("name".to_owned(), "kid".into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("tree".to_owned(), "given".into());
    rig.family.host.spawn("look".to_owned(), asked)?;
    assert!(rig.family.reaches("kid", "finished").await);
    let refused = rig
        .ask("tree.discard", json!({"tree": "given"}))
        .await
        .err()
        .ok_or("a child's tree was discarded under it")?;
    assert!(refused.contains("discard_worktree"), "{refused}");
    assert!(kept.exists(), "the child's tree is still there");
    Ok(())
}

/// Dies with `merge_worktree` taking a dirty fork: the child's commit would claim a person's edits.
#[tokio::test]
async fn a_child_on_a_dirty_fork_cannot_merge_by_commit_and_merges_by_tree() -> TestResult {
    let rig = rig("given")?;
    let at = finished_kid_on_a_dirty_fork(&rig).await?;
    assert_eq!(rig.family.host.cwd_of("kid"), Some(at.clone()));
    let refused = rig
        .family
        .host
        .merge_worktree("kid", None)
        .err()
        .ok_or("a dirty fork was merged")?;
    assert!(refused.contains("rlm.tree().merge(h.tree)"), "{refused}");
    if !foldable(&rig)? {
        let why = rig
            .ask("tree.merge", json!({"tree": "self", "others": ["kid"]}))
            .await
            .err()
            .ok_or("a fold ran on a git that cannot fold")?;
        assert!(why.contains("needs git 2.43"), "{why}");
        return Ok(());
    }
    std::fs::write(at.join("f1.txt"), "kid's\n")?;
    let reply = rig
        .ask("tree.merge", json!({"tree": "self", "others": ["kid"]}))
        .await?;
    assert_eq!(reply["summary"], "merged 1 of 1", "{reply:?}");
    assert_eq!(read(&rig.repo.join("f1.txt"))?, "kid's\n");
    rig.family.host.delete("kid")?;
    assert!(!at.exists(), "the folded child's tree went with it");
    Ok(())
}

/// Dies with the fold discarding past a live job: a finished child's background command still
/// ran in the tree the fold settled, and the slot it held was released under it.
#[tokio::test]
async fn a_fold_leaves_a_tree_with_a_live_job_in_place_for_the_named_release() -> TestResult {
    let rig = rig("fold-live-job")?;
    let at = finished_kid_on_a_dirty_fork(&rig).await?;
    if !foldable(&rig)? {
        return Ok(());
    }
    std::fs::write(at.join("f1.txt"), "kid's\n")?;
    let never: yi_tools::CancelFlag = std::sync::Arc::new(|| false);
    yi_tools::jobs::spawn_job("setsid sleep 3", &at, &never, None, None)?;
    let reply = rig
        .ask("tree.merge", json!({"tree": "self", "others": ["kid"]}))
        .await?;
    let note = (reply.get("notes").and_then(Value::as_array))
        .and_then(|notes| notes.first())
        .and_then(Value::as_str)
        .ok_or("a tree with a live job was released without a note")?;
    assert!(note.contains("kid"), "{note}");
    assert!(
        note.contains("rlm.discard_worktree(\"kid\", force=True)"),
        "{note}"
    );
    assert!(at.exists(), "the tree was discarded under a live job");
    support::wait_until(15_000, || {
        yi_tools::jobs::registry().running_under(&at).is_empty()
    })
    .await;
    let again = rig
        .ask(
            "rlm.discard_worktree",
            json!({"target": "kid", "force": true}),
        )
        .await?;
    assert_eq!(again["discarded"], true, "{again:?}");
    assert!(
        !at.exists(),
        "the named release kept a tree whose jobs had finished"
    );
    Ok(())
}

#[tokio::test]
async fn a_managed_child_fold_keeps_tree_and_names_discard_refusal() -> TestResult {
    let rig = rig("managed-fold")?;
    let at = finished_kid_on_a_dirty_fork(&rig).await?;
    std::fs::write(at.join("f1.txt"), "kid's\n")?;
    read_version_as(&rig, "git version 2.43.0", MergeTree::Succeeds)?;
    rig.family.host.mark_managed("kid")?;
    let reply = rig
        .ask("tree.merge", json!({"tree": "self", "others": ["kid"]}))
        .await?;
    let notes = reply
        .get("notes")
        .and_then(Value::as_array)
        .ok_or("a managed child's discard refusal was not named")?;
    let note = notes
        .first()
        .and_then(Value::as_str)
        .ok_or("the managed child had no discard-refusal note")?;
    assert!(note.contains("kid"), "{note}");
    assert!(note.contains("was dispatched by the plan"), "{note}");
    assert!(at.exists(), "the managed child's tree was discarded");
    assert!(note.contains("tree was left in place"), "{note}");
    Ok(())
}

async fn finished_kid_on_a_dirty_fork(rig: &Rig) -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::fs::write(rig.repo.join("f0.txt"), "a person's\n")?;
    let at = rig.fork("given").await?;
    let mut asked = Map::new();
    asked.insert("name".to_owned(), "kid".into());
    asked.insert("role".to_owned(), "root".into());
    asked.insert("tree".to_owned(), "given".into());
    rig.family.host.spawn("look".to_owned(), asked)?;
    assert!(rig.family.reaches("kid", "finished").await);
    Ok(at)
}

/// A child that never touched its fork holds only the files its parent carried in: discarding it
/// loses nothing, so a failed child can always be released.
#[tokio::test]
async fn a_dirty_fork_the_child_never_changed_discards_without_force() -> TestResult {
    let rig = rig("given")?;
    let at = finished_kid_on_a_dirty_fork(&rig).await?;
    rig.family.host.discard_worktree("kid", false)?;
    assert!(!at.exists());
    rig.family.host.delete("kid")?;
    Ok(())
}

/// A dirty fork the child wrote to refuses a plain discard, names what drops it, and yields to force.
#[tokio::test]
async fn a_dirty_fork_the_child_changed_discards_only_with_force() -> TestResult {
    let rig = rig("given")?;
    let at = finished_kid_on_a_dirty_fork(&rig).await?;
    std::fs::write(at.join("kid.txt"), "kid's own work\n")?;
    let refused = rig
        .ask("rlm.discard_worktree", json!({"target": "kid"}))
        .await
        .err()
        .ok_or("a dirty fork the child wrote to was discarded without force")?;
    assert!(refused.contains("force=True"), "{refused}");
    assert!(at.exists());
    rig.ask(
        "rlm.discard_worktree",
        json!({"target": "kid", "force": true}),
    )
    .await?;
    assert!(!at.exists());
    Ok(())
}

/// A source must read the same from every role or refuse by name: green suites shipped a reader
/// resolver that could open no session but its own. Root, reader, worker and walled.
#[tokio::test]
async fn tree_rows_read_the_same_from_every_role_and_a_wall_refuses_by_name() -> TestResult {
    use yi_runtime::fetch::{FetchError, MemberTrees, ParentTrees, Resolver};
    use yi_runtime::wall::Wall;
    let rig = rig("roles")?;
    let at = rig.fork("seen").await?;
    std::fs::write(at.join("new.txt"), "x\n")?;
    let host = std::sync::Arc::clone(&rig.family.host);
    let rooted: std::sync::Arc<dyn MemberTrees> = host.clone();
    let reading: std::sync::Arc<dyn MemberTrees> =
        std::sync::Arc::new(ParentTrees(std::sync::Arc::downgrade(&host)));
    let (rows, file) = ("tree://seen/".parse()?, "tree://seen/new.txt".parse()?);
    for (role, trees) in [
        ("root", &rooted),
        ("reader", &reading),
        ("worker", &reading),
    ] {
        let resolver = Resolver::new(rig.repo.clone(), Wall::default())
            .with_member_trees(std::sync::Arc::clone(trees));
        let listed = resolver
            .fetch(&rows)
            .map_err(|error| format!("{role}: {error:?}"))?;
        assert_eq!(listed.text, "+1 -0 new.txt", "{role}");
        let opened = resolver
            .fetch(&file)
            .map_err(|error| format!("{role}: {error:?}"))?;
        assert_eq!(opened.text, "x\n", "{role}");
    }
    let walled = Wall {
        deny_url: vec!["tree://".to_owned()],
        ..Wall::default()
    };
    let resolver = Resolver::new(rig.repo.clone(), walled).with_member_trees(rooted);
    assert!(
        matches!(resolver.fetch(&rows), Err(FetchError::Denied { .. })),
        "a wall names itself"
    );
    let bare = Resolver::new(rig.repo.clone(), Wall::default());
    assert!(
        bare.fetch(&rows).is_err(),
        "no family is a refusal, never an empty list"
    );
    Ok(())
}

#[tokio::test]
async fn a_check_wait_of_any_float_is_answered_never_a_panic() -> TestResult {
    let rig = rig("waitlimit")?;
    rig.fork("w").await?;
    let waits = [
        json!(30.0),
        json!(590.0),
        json!(591.0),
        json!(1e30),
        json!(f64::MAX),
        json!(-1.0),
        json!("soon"),
        json!(null),
    ];
    for wait in waits {
        let reply = rig
            .ask(
                "tree.check",
                json!({"tree": "w", "cmd": "true", "wait_s": wait}),
            )
            .await?;
        assert_eq!(reply["state"], "green", "wait_s {wait}: {reply:?}");
    }
    Ok(())
}
