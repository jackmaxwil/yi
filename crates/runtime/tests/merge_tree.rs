//! `merge-tree` against the reference that did not come from it: `git merge` in a staging lane, run
//! on the same commits. A case agrees on the tree id, or on every conflict's path and side.
use crate::git_lane::git;
use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use yi_runtime::lane::Pool;
use yi_runtime::lane::Sha;
use yi_runtime::lane::settle::{Prepared, generation_of, prepare};
use yi_runtime::lane::stage::{folds, git_version, merge_tree_rows, merged};

type TestResult = Result<(), Box<dyn Error>>;

struct Rig {
    _root: Scratch,
    repo: PathBuf,
    home: PathBuf,
}

fn write(repo: &Path, path: &str, bytes: &[u8]) -> TestResult {
    let at = repo.join(path);
    if let Some(up) = at.parent() {
        std::fs::create_dir_all(up)?;
    }
    Ok(std::fs::write(at, bytes)?)
}

fn commit(repo: &Path, message: &str) -> TestResult {
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "--allow-empty", "-m", message])?;
    Ok(())
}

/// `base` on main, `theirs` on branch `candidate`, then `ours` on main.
fn rig(
    tag: &str,
    base: &[(&str, &[u8])],
    ours: &dyn Fn(&Path) -> TestResult,
    theirs: &dyn Fn(&Path) -> TestResult,
) -> Result<Rig, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-mt-{tag}"))?;
    let (repo, home) = (root.join("repo"), root.join("home"));
    std::fs::create_dir_all(&repo)?;
    std::fs::create_dir_all(&home)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    for (path, bytes) in base {
        write(&repo, path, bytes)?;
    }
    commit(&repo, "base")?;
    git(&repo, &["checkout", "-q", "-b", "candidate"])?;
    theirs(&repo)?;
    commit(&repo, "theirs")?;
    git(&repo, &["checkout", "-q", "main"])?;
    ours(&repo)?;
    commit(&repo, "ours")?;
    Ok(Rig {
        _root: root,
        repo,
        home,
    })
}

type Answer = Result<String, Vec<(String, String)>>;

fn answer(prepared: Prepared, repo: &Path) -> Result<Answer, Box<dyn Error>> {
    match prepared {
        Prepared::Merged(staging) => {
            let tree = git(
                repo,
                &[
                    "rev-parse",
                    &format!("{}^{{tree}}", staging.integrated.as_str()),
                ],
            )?;
            let parents = git(
                repo,
                &[
                    "rev-list",
                    "--parents",
                    "-n",
                    "1",
                    staging.integrated.as_str(),
                ],
            )?;
            assert!(
                parents.contains(staging.base.as_str())
                    && parents.contains(staging.candidate.as_str()),
                "{parents}"
            );
            staging.release()?;
            Ok(Ok(tree))
        }
        Prepared::Conflict { conflicts, .. } => {
            let mut rows: Vec<(String, String)> = conflicts
                .into_iter()
                .map(|row| (row.path, row.side))
                .collect();
            rows.sort();
            Ok(Err(rows))
        }
    }
}

fn agree(rig: &Rig) -> Result<Answer, Box<dyn Error>> {
    let pool = Pool::open(&rig.home, &rig.repo, 2)?;
    let generation = generation_of(&rig.repo, None)?;
    let tip = git(&rig.repo, &["rev-parse", "candidate"])?;
    let candidate = Sha::parse(&tip).ok_or("no sha")?;
    let native = merged(&pool, &rig.repo, "s-native", &generation, &candidate, None)?;
    let can_fold = folds(&git(&rig.repo, &["--version"])?);
    if let (Prepared::Merged(staging), true) = (&native, can_fold) {
        assert_eq!(staging.path(), None, "a merge in the store holds no lane");
    }
    let reference = prepare(&pool, "s-reference", &generation, &candidate, None)?;
    let (native, reference) = (answer(native, &rig.repo)?, answer(reference, &rig.repo)?);
    assert_eq!(native, reference, "merge-tree and git merge disagree");
    Ok(native)
}

fn edit(path: &'static str, bytes: &'static [u8]) -> impl Fn(&Path) -> TestResult {
    move |repo| write(repo, path, bytes)
}

#[test]
fn a_clean_merge_agrees_and_touches_no_checkout() -> TestResult {
    let rig = rig(
        "clean",
        &[("a", b"a\n"), ("b", b"b\n")],
        &edit("a", b"ours\n"),
        &edit("b", b"theirs\n"),
    )?;
    let head = git(&rig.repo, &["rev-parse", "HEAD"])?;
    assert!(agree(&rig)?.is_ok());
    assert_eq!(git(&rig.repo, &["rev-parse", "HEAD"])?, head);
    assert_eq!(git(&rig.repo, &["status", "--porcelain"])?, "");
    Ok(())
}

#[test]
fn a_union_attribute_at_the_base_merges_both_sides() -> TestResult {
    let base: &[(&str, &[u8])] = &[
        (".gitattributes", b"list merge=union\n"),
        ("list", b"one\n"),
    ];
    let rig = rig(
        "union",
        base,
        &edit("list", b"one\nours\n"),
        &edit("list", b"one\ntheirs\n"),
    )?;
    assert!(agree(&rig)?.is_ok());
    Ok(())
}

#[test]
fn a_merge_driver_in_the_project_config_runs_on_both_roads() -> TestResult {
    let base: &[(&str, &[u8])] = &[(".gitattributes", b"lock merge=keep\n"), ("lock", b"v1\n")];
    let rig = rig(
        "driver",
        base,
        &edit("lock", b"ours\n"),
        &edit("lock", b"theirs\n"),
    )?;
    git(&rig.repo, &["config", "merge.keep.driver", "true"])?;
    assert!(agree(&rig)?.is_ok());
    Ok(())
}

#[test]
fn a_parent_with_no_attributes_ignores_the_ones_the_candidate_brings() -> TestResult {
    let theirs = |repo: &Path| {
        write(repo, ".gitattributes", b"list merge=union\n")?;
        write(repo, "list", b"one\ntheirs\n")
    };
    let rig = rig(
        "attr-source",
        &[("list", b"one\n")],
        &edit("list", b"one\nours\n"),
        &theirs,
    )?;
    assert_eq!(
        agree(&rig)?,
        Err(vec![("list".to_owned(), "both".to_owned())])
    );
    Ok(())
}

#[test]
fn a_rename_on_one_side_follows_an_edit_on_the_other() -> TestResult {
    let text: &[u8] = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";
    let ours = |repo: &Path| git(repo, &["mv", "a", "b"]).map(drop);
    let rig = rig(
        "rename",
        &[("a", text)],
        &ours,
        &edit("a", b"one\ntwo\nthree\nfour\nfive\nsix\nseven\nEIGHT\n"),
    )?;
    assert!(agree(&rig)?.is_ok());
    Ok(())
}

#[test]
fn delete_against_modify_is_one_conflict_on_the_side_that_kept_it() -> TestResult {
    let gone = |repo: &Path| Ok(std::fs::remove_file(repo.join("a"))?);
    let rig = rig("delete", &[("a", b"a\n")], &gone, &edit("a", b"theirs\n"))?;
    assert_eq!(
        agree(&rig)?,
        Err(vec![("a".to_owned(), "theirs".to_owned())])
    );
    Ok(())
}

#[test]
fn modify_against_delete_is_one_conflict_on_our_side() -> TestResult {
    let gone = |repo: &Path| Ok(std::fs::remove_file(repo.join("a"))?);
    let rig = rig(
        "delete-theirs",
        &[("a", b"a\n")],
        &edit("a", b"ours\n"),
        &gone,
    )?;
    assert_eq!(agree(&rig)?, Err(vec![("a".to_owned(), "ours".to_owned())]));
    Ok(())
}

#[test]
fn add_against_add_and_a_binary_edit_conflict_on_both_sides() -> TestResult {
    let both = rig(
        "add",
        &[("a", b"a\n")],
        &edit("new", b"ours\n"),
        &edit("new", b"theirs\n"),
    )?;
    assert_eq!(
        agree(&both)?,
        Err(vec![("new".to_owned(), "both".to_owned())])
    );
    let binary = rig(
        "binary",
        &[("b", b"\0base")],
        &edit("b", b"\0ours"),
        &edit("b", b"\0theirs"),
    )?;
    assert_eq!(
        agree(&binary)?,
        Err(vec![("b".to_owned(), "both".to_owned())])
    );
    Ok(())
}

/// A criss-cross has two merge bases: passing one by hand with `--merge-base` would disagree here.
#[test]
fn a_criss_cross_history_agrees() -> TestResult {
    let rig = rig(
        "criss",
        &[("a", b"1\n2\n3\n"), ("b", b"b\n")],
        &edit("a", b"1\n2\nours\n"),
        &edit("b", b"theirs\n"),
    )?;
    let repo = &rig.repo;
    git(repo, &["branch", "side-a", "main"])?;
    git(repo, &["branch", "side-b", "candidate"])?;
    git(repo, &["checkout", "-q", "side-a"])?;
    git(repo, &["merge", "-q", "--no-edit", "candidate"])?;
    git(repo, &["checkout", "-q", "side-b"])?;
    git(repo, &["merge", "-q", "--no-edit", "main"])?;
    write(repo, "c", b"from b\n")?;
    commit(repo, "more on b")?;
    git(repo, &["branch", "-f", "candidate", "side-b"])?;
    git(repo, &["checkout", "-q", "main"])?;
    git(repo, &["merge", "-q", "--no-edit", "side-a"])?;
    write(repo, "d", b"from a\n")?;
    commit(repo, "more on a")?;
    assert!(agree(&rig)?.is_ok());
    Ok(())
}

#[test]
fn a_broken_candidate_is_an_error_and_never_a_conflict() -> TestResult {
    let rig = rig(
        "broken",
        &[("a", b"a\n")],
        &edit("a", b"o\n"),
        &edit("a", b"t\n"),
    )?;
    let pool = Pool::open(&rig.home, &rig.repo, 2)?;
    let generation = generation_of(&rig.repo, None)?;
    let ghost = Sha::parse("0123456789abcdef0123456789abcdef01234567").ok_or("no sha")?;
    assert!(merged(&pool, &rig.repo, "s", &generation, &ghost, None).is_err());
    Ok(())
}

#[test]
fn a_version_is_read_or_it_is_not_new_enough() {
    assert_eq!(
        git_version("git version 2.54.0 (Apple Git-157)\n"),
        Some((2, 54))
    );
    assert_eq!(git_version("git version 2.43.0"), Some((2, 43)));
    assert_eq!(git_version("git version 2.42.9"), Some((2, 42)));
    assert_eq!(git_version("git version 2.43-rc0"), None);
    assert_eq!(git_version("hub version 2.14"), None);
    assert!(Some((2, 42)) < Some((2, 43)) && Some((3, 0)) > Some((2, 43)));
}

#[test]
fn conflict_rows_read_to_the_empty_field_and_no_further() {
    let out = "tree\x00100644 aaa 2\tp\x00100644 bbb 3\tp\x00\x001\x00p\x00CONFLICT\x00text\x00";
    let rows = merge_tree_rows(out);
    assert_eq!(rows, Some(vec![("p".to_owned(), 2), ("p".to_owned(), 3)]));
    assert_eq!(merge_tree_rows(""), None);
    assert_eq!(merge_tree_rows("tree\x00no tab here\x00"), None);
}

proptest! {
    #[test]
    fn no_text_panics_the_row_reader(text in ".{0,200}") {
        let _rows = merge_tree_rows(&text);
        let _version = git_version(&text);
    }
}
