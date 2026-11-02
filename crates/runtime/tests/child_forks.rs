//! A worktree child's tree: cut at HEAD, cheap on this filesystem, and carrying only what cannot
//! name its parent. Fixtures are real repositories and real cargo builds; `.d` files are also written
//! by hand where the test is the limit, which a contained process could write.
use crate::git_lane::git;
use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use yi_runtime::lane::Pool;
use yi_runtime::lane::carry::{Carried, carry_cargo, carry_includes, dep_info, include_list};
use yi_runtime::lane::fork::{Carry, ChildTree, forks_dir, is_held, parent_state, reclaim};

type TestResult = Result<(), Box<dyn Error>>;

struct Rig {
    root: Scratch,
    repo: PathBuf,
    home: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let root = Scratch::new(&format!("yi-fork-{tag}"))?;
        let (repo, home) = (root.join("repo"), root.join("home"));
        std::fs::create_dir_all(repo.join("src/deep"))?;
        std::fs::create_dir_all(&home)?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        std::fs::write(
            repo.join(".gitignore"),
            "target/\n.env*\nsecret/\n.venv/\n*.note\nsub/.venv/\n",
        )?;
        std::fs::write(repo.join("src/deep/a.txt"), "tracked\n")?;
        std::fs::write(repo.join("README.md"), "first\n")?;
        std::os::unix::fs::symlink("deep/a.txt", repo.join("src/link"))?;
        git(&repo, &["add", "-A"])?;
        git(&repo, &["commit", "-q", "-m", "first"])?;
        Ok(Self { root, repo, home })
    }

    fn pool(&self) -> Result<Pool, Box<dyn Error>> {
        Ok(Pool::open(&self.home, &self.repo, 1)?)
    }

    fn fork(&self, id: &str) -> Result<(ChildTree, Vec<String>), Box<dyn Error>> {
        Ok(ChildTree::fork(
            &self.pool()?,
            &self.repo,
            id,
            &Carry::default(),
        )?)
    }
}

#[test]
fn the_child_is_a_clean_tree_at_head_and_writes_reach_only_it() -> TestResult {
    let rig = Rig::new("clean")?;
    let (tree, _rows) = rig.fork("c1")?;
    let at = tree.path();
    assert_eq!(
        std::fs::read_to_string(at.join("src/deep/a.txt"))?,
        "tracked\n"
    );
    assert_eq!(
        std::fs::read_link(at.join("src/link"))?,
        Path::new("deep/a.txt")
    );
    assert_eq!(
        git(at, &["status", "--porcelain"])?,
        "",
        "a fork starts clean"
    );
    assert_eq!(
        git(at, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        tree.branch().as_str()
    );
    std::fs::write(at.join("src/deep/a.txt"), "the child's\n")?;
    assert_eq!(
        std::fs::read_to_string(rig.repo.join("src/deep/a.txt"))?,
        "tracked\n"
    );
    Ok(())
}

#[test]
fn a_session_in_a_subdirectory_still_forks_the_whole_repository() -> TestResult {
    let rig = Rig::new("subdir")?;
    let inside = rig.repo.join("src");
    let (tree, _rows) = ChildTree::fork(&rig.pool()?, &inside, "c1", &Carry::default())?;
    assert!(tree.path().join("README.md").is_file());
    assert!(tree.path().join("src/deep/a.txt").is_file());
    assert!(
        !tree.path().join("deep").exists(),
        "the subdirectory stayed where it was"
    );
    Ok(())
}

#[test]
fn a_dirty_parent_stays_behind_and_the_candidate_is_only_the_childs() -> TestResult {
    let rig = Rig::new("dirty")?;
    std::fs::write(rig.repo.join("README.md"), "a person's edit\n")?;
    std::fs::write(rig.repo.join("scratch.txt"), "untracked\n")?;
    let (tree, rows) = rig.fork("c1")?;
    let at = tree.path();
    assert_eq!(std::fs::read_to_string(at.join("README.md"))?, "first\n");
    assert!(!at.join("scratch.txt").exists());
    assert!(
        rows.iter()
            .any(|row| row.contains("without 2 uncommitted paths")),
        "{rows:?}"
    );
    std::fs::write(at.join("mine.txt"), "child\n")?;
    let quiet = yi_runtime::lane::settle::Quiescence {
        at: 0,
        running: Vec::new(),
    };
    let candidate = tree.candidate(&quiet, None)?;
    let files = git(
        at,
        &[
            "diff",
            "--name-only",
            tree.base().as_str(),
            candidate.commit.as_str(),
        ],
    )?;
    assert_eq!(files, "mine.txt");
    assert_eq!(
        std::fs::read_to_string(rig.repo.join("README.md"))?,
        "a person's edit\n"
    );
    Ok(())
}

#[test]
fn ignored_state_stays_out_nested_and_at_the_root() -> TestResult {
    let rig = Rig::new("ignored")?;
    std::fs::create_dir_all(rig.repo.join("src/deep/.venv/bin"))?;
    std::fs::write(rig.repo.join("src/deep/.venv/bin/python"), "x")?;
    std::fs::write(rig.repo.join(".env"), "KEY=1\n")?;
    std::fs::create_dir_all(rig.repo.join("target/debug"))?;
    std::fs::write(rig.repo.join("target/debug/app"), "x")?;
    let (tree, _rows) = rig.fork("c1")?;
    let at = tree.path();
    assert!(
        at.join("src/deep/a.txt").is_file(),
        "the directory holding it still came"
    );
    assert!(!at.join("src/deep/.venv").exists());
    assert!(!at.join(".env").exists());
    assert!(!at.join("target").exists());
    Ok(())
}

#[test]
fn the_scratch_row_names_three_and_cuts_at_the_fourth() -> TestResult {
    let rig = Rig::new("rows")?;
    for name in ["a.note", "b.note", "c.note"] {
        std::fs::write(rig.repo.join(name), "x")?;
    }
    let (_tree, rows) = rig.fork("c1")?;
    let text = rows.join("\n");
    assert!(
        text.contains("ignored and not in the child: a.note, b.note, c.note"),
        "{text}"
    );
    assert!(!text.contains("[…"), "three fit: {text}");
    assert!(text.contains("carry=[\".env\"]"), "{text}");
    std::fs::write(rig.repo.join("d.note"), "x")?;
    let (_second, rows) = rig.fork("c2")?;
    let text = rows.join("\n");
    assert!(text.contains("3 of 4 ignored paths named"), "{text}");
    assert!(text.contains("fork.scratch_rows"), "{text}");
    assert!(text.contains("git status --ignored"), "{text}");
    Ok(())
}

#[test]
fn a_carried_file_is_not_reported_as_missing() -> TestResult {
    let rig = Rig::new("carried")?;
    std::fs::write(rig.repo.join(".env"), "KEY=1\n")?;
    std::fs::write(rig.repo.join("a.note"), "x")?;
    let plan = Carry {
        extra: vec![".env".to_owned()],
        ..Carry::default()
    };
    let (tree, rows) = ChildTree::fork(&rig.pool()?, &rig.repo, "c1", &plan)?;
    assert_eq!(
        std::fs::read_to_string(tree.path().join(".env"))?,
        "KEY=1\n"
    );
    let text = rows.join("\n");
    assert!(
        text.contains("ignored and not in the child: a.note"),
        "{text}"
    );
    assert!(!text.contains("not in the child: .env"), "{text}");
    Ok(())
}

#[test]
fn a_repository_with_submodules_takes_the_plain_rung_and_stays_clean() -> TestResult {
    let rig = Rig::new("submodule")?;
    let inner = rig.root.join("inner");
    std::fs::create_dir_all(&inner)?;
    git(&inner, &["init", "-q", "-b", "main"])?;
    git(&inner, &["commit", "-q", "--allow-empty", "-m", "inner"])?;
    git(
        &rig.repo,
        &[
            "submodule",
            "add",
            "-q",
            inner.to_string_lossy().as_ref(),
            "vendored",
        ],
    )?;
    git(&rig.repo, &["commit", "-q", "-m", "submodule"])?;
    let (tree, _rows) = rig.fork("c1")?;
    assert!(tree.path().join(".gitmodules").is_file());
    assert_eq!(git(tree.path(), &["status", "--porcelain"])?, "");
    Ok(())
}

#[test]
fn slots_do_not_bound_forks() -> TestResult {
    let rig = Rig::new("slots")?;
    let held: Vec<_> = (0..5)
        .map(|n| rig.fork(&format!("c{n}")))
        .collect::<Result<_, _>>()?;
    assert_eq!(held.len(), 5, "a pool of one slot still forks five");
    Ok(())
}

#[test]
fn the_flock_outlives_a_replaced_record_and_ends_with_its_holder() -> TestResult {
    let rig = Rig::new("held")?;
    let (tree, _rows) = rig.fork("c1")?;
    let path = tree.path().to_path_buf();
    assert!(is_held(&path));
    yi_session::replace_file(&path.with_extension("json"), b"{}")?;
    assert!(
        is_held(&path),
        "a rename beside the lock leaves its inode alone"
    );
    drop(tree);
    assert!(!is_held(&path));
    Ok(())
}

#[test]
fn a_dead_holders_work_lands_on_its_branch_before_the_tree_goes() -> TestResult {
    let rig = Rig::new("reclaim")?;
    let (tree, _rows) = rig.fork("c1")?;
    let (path, branch) = (tree.path().to_path_buf(), tree.branch().as_str().to_owned());
    std::fs::write(path.join("work.txt"), "kept\n")?;
    drop(tree);
    assert!(reclaim(&rig.home, &path));
    assert!(!path.exists());
    assert_eq!(
        git(&rig.repo, &["show", &format!("{branch}:work.txt")])?,
        "kept"
    );
    Ok(())
}

#[test]
fn gc_and_retention_keep_a_live_fork() -> TestResult {
    let rig = Rig::new("keep")?;
    let (tree, _rows) = rig.fork("c1")?;
    let _removed = rig.pool()?.gc(&rig.repo, false)?;
    let _swept = yi_runtime::retention::sweep(&rig.home, false);
    assert!(tree.path().join("README.md").is_file());
    assert_eq!(forks_dir(&rig.home), rig.home.join(".yi/forks"));
    Ok(())
}

/// A profile by hand: `hash` names one unit across `.fingerprint`, `build` and `deps`.
fn unit(profile: &Path, package: &str, krate: &str, hash: &str, d: &[u8]) -> TestResult {
    let (fingerprint, build, deps) = (
        profile.join(".fingerprint"),
        profile.join("build"),
        profile.join("deps"),
    );
    std::fs::create_dir_all(fingerprint.join(format!("{package}-{hash}")))?;
    std::fs::create_dir_all(build.join(format!("{package}-{hash}")))?;
    std::fs::create_dir_all(&deps)?;
    std::fs::write(deps.join(format!("lib{krate}-{hash}.rlib")), "rlib")?;
    std::fs::write(deps.join(format!("{krate}-{hash}.d")), d)?;
    Ok(())
}

type CarryRig = (Scratch, PathBuf, PathBuf, PathBuf);

fn carry_rig(tag: &str) -> Result<CarryRig, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-carry-{tag}"))?;
    let (source, tree) = (root.join("source"), root.join("tree"));
    let profile = source.join("target/debug");
    std::fs::create_dir_all(&profile)?;
    std::fs::create_dir_all(&tree)?;
    std::fs::write(tree.join("Cargo.lock"), "")?;
    Ok((root, source, tree, profile))
}

const HASH_A: &str = "0123456789abcdef";
const HASH_B: &str = "fedcba9876543210";
const HASH_C: &str = "aaaaaaaaaaaaaaaa";

#[test]
fn a_unit_naming_the_tree_drops_its_whole_package_and_others_carry() -> TestResult {
    let (_root, source, tree, profile) = carry_rig("classes")?;
    let elsewhere = b"/elsewhere/lib.rs: /elsewhere/lib.rs\n";
    let relative = b"lib.rs: crates/app/src/lib.rs\n";
    unit(&profile, "third-party", "third_party", HASH_A, elsewhere)?;
    unit(&profile, "app", "renamed_lib", HASH_B, relative)?;
    // The build-script run of the same package has no dep-info of its own.
    std::fs::create_dir_all(profile.join(format!(".fingerprint/app-{HASH_C}")))?;
    std::fs::create_dir_all(profile.join(format!("build/app-{HASH_C}")))?;
    assert_eq!(
        carry_cargo(&source, &tree, "c1"),
        Carried::Units(4),
        "fingerprint, build, rlib and .d"
    );
    let carried = tree.join("target/debug");
    assert!(
        carried
            .join(format!("deps/libthird_party-{HASH_A}.rlib"))
            .is_file()
    );
    assert!(
        carried
            .join(format!(".fingerprint/third-party-{HASH_A}"))
            .is_dir()
    );
    assert!(
        !carried
            .join(format!("deps/librenamed_lib-{HASH_B}.rlib"))
            .exists(),
        "crate name is not package name"
    );
    assert!(!carried.join(format!(".fingerprint/app-{HASH_B}")).exists());
    assert!(
        !carried.join(format!("build/app-{HASH_C}")).exists(),
        "the run unit went with its package"
    );
    Ok(())
}

#[test]
fn an_absolute_path_under_the_source_names_the_tree_and_its_target_does_not() -> TestResult {
    let (_root, source, tree, profile) = carry_rig("absolute")?;
    let inside = format!("x.rs: {}/vendor/syntect/lib.rs\n", source.display());
    let built = format!("x.rs: {}/target/debug/build/out.rs\n", source.display());
    unit(&profile, "syntect", "syntect", HASH_A, inside.as_bytes())?;
    unit(&profile, "generated", "generated", HASH_B, built.as_bytes())?;
    assert!(matches!(
        carry_cargo(&source, &tree, "c1"),
        Carried::Units(_)
    ));
    let deps = tree.join("target/debug/deps");
    assert!(!deps.join(format!("libsyntect-{HASH_A}.rlib")).exists());
    assert!(deps.join(format!("libgenerated-{HASH_B}.rlib")).is_file());
    Ok(())
}

#[test]
fn a_dep_info_at_its_limit_is_read_and_one_past_drops_the_unit() -> TestResult {
    let limit = 4usize << 20;
    let body = |len: usize| {
        let mut bytes = b"x: /elsewhere/a.rs\n#".to_vec();
        bytes.resize(len, b' ');
        bytes
    };
    let (_root, source, tree, profile) = carry_rig("limit")?;
    unit(&profile, "fits", "fits", HASH_A, &body(limit))?;
    unit(&profile, "over", "over", HASH_B, &body(limit + 1))?;
    assert!(matches!(
        carry_cargo(&source, &tree, "c1"),
        Carried::Units(_)
    ));
    let deps = tree.join("target/debug/deps");
    assert!(deps.join(format!("libfits-{HASH_A}.rlib")).is_file());
    assert!(!deps.join(format!("libover-{HASH_B}.rlib")).exists());
    Ok(())
}

#[test]
fn the_dep_info_count_holds_at_its_limit_and_refuses_one_past() -> TestResult {
    let (_root, source, tree, profile) = carry_rig("count")?;
    let deps = profile.join("deps");
    std::fs::create_dir_all(&deps)?;
    for n in 0..65_536 {
        std::fs::write(deps.join(format!("n{n}.d")), "")?;
    }
    assert!(matches!(
        carry_cargo(&source, &tree, "c1"),
        Carried::Units(_)
    ));
    std::fs::remove_dir_all(tree.join("target"))?;
    std::fs::write(deps.join("one-more.d"), "")?;
    let Carried::Refused(row) = carry_cargo(&source, &tree, "c2") else {
        return Err("one past the count carried".into());
    };
    assert!(row.contains("fork.depinfo_files 65536"), "{row}");
    assert!(!tree.join("target").exists());
    Ok(())
}

#[test]
fn nothing_carries_where_the_child_already_built_or_no_cargo_project_is() -> TestResult {
    let (_root, source, tree, profile) = carry_rig("nothing")?;
    unit(&profile, "a", "a", HASH_A, b"")?;
    std::fs::create_dir_all(tree.join("target"))?;
    assert_eq!(carry_cargo(&source, &tree, "c1"), Carried::Nothing);
    std::fs::remove_dir_all(tree.join("target"))?;
    std::fs::remove_file(tree.join("Cargo.lock"))?;
    assert_eq!(carry_cargo(&source, &tree, "c1"), Carried::Nothing);
    Ok(())
}

fn cargo(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let out = yi_tools::keyless_command("cargo")
        .args(args)
        .env_remove("CARGO_TARGET_DIR")
        .env("CARGO_INCREMENTAL", "0")
        .current_dir(dir)
        .output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("cargo {args:?}: {text}").into());
    }
    Ok(text)
}

fn write(path: &Path, text: &str) -> TestResult {
    if let Some(up) = path.parent() {
        std::fs::create_dir_all(up)?;
    }
    Ok(std::fs::write(path, text)?)
}

/// The shapes a name list got wrong: a member, and a `[patch]`ed and excluded crate in the tree. Each
/// bakes `CARGO_MANIFEST_DIR`; `third` lies outside, and is what may carry.
fn cargo_fixture(rig: &Rig) -> TestResult {
    let repo = &rig.repo;
    let outside = rig.root.join("outside");
    let bake = "pub fn at() -> &'static str { env!(\"CARGO_MANIFEST_DIR\") }\n";
    write(
        &outside.join("third/Cargo.toml"),
        "[package]\nname = \"third\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
    )?;
    write(&outside.join("third/src/lib.rs"), bake)?;
    write(
        &repo.join("Cargo.toml"),
        &format!(
            "[workspace]\nmembers = [\"member\", \"app\"]\nexclude = [\"vendor/patched\"]\nresolver = \"2\"\n[patch.crates-io]\npatched = {{ path = \"vendor/patched\" }}\n[workspace.dependencies]\nthird = {{ path = \"{0}/third\" }}\n",
            outside.display()
        ),
    )?;
    write(
        &repo.join("member/Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    write(&repo.join("member/src/lib.rs"), bake)?;
    write(
        &repo.join("vendor/patched/Cargo.toml"),
        "[package]\nname = \"patched\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
    )?;
    write(&repo.join("vendor/patched/src/lib.rs"), bake)?;
    write(
        &repo.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nmember = { path = \"../member\" }\npatched = { path = \"../vendor/patched\" }\nthird.workspace = true\n",
    )?;
    write(
        &repo.join("app/src/main.rs"),
        "fn main() { println!(\"{} {}\", member::at(), patched::at()); }\n",
    )?;
    cargo(repo, &["build", "--offline", "--quiet"])?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "-m", "cargo"])?;
    Ok(())
}

type Fingerprints = Vec<(PathBuf, Vec<u8>)>;

fn fingerprints(profile: &Path) -> Result<Fingerprints, Box<dyn Error>> {
    let mut found = Vec::new();
    for dir in std::fs::read_dir(profile.join(".fingerprint"))? {
        for file in std::fs::read_dir(dir?.path())? {
            let path = file?.path();
            found.push((path.clone(), std::fs::read(path)?));
        }
    }
    found.sort();
    Ok(found)
}

/// Dies with a name list, or with a carried in-tree unit: cargo says Fresh, and the child's binary
/// names the parent's tree while its tests pass against the parent's code.
#[test]
fn a_child_builds_its_own_tree_and_reuses_only_what_names_nothing_in_it() -> TestResult {
    let rig = Rig::new("baked")?;
    cargo_fixture(&rig)?;
    let before = fingerprints(&rig.repo.join("target/debug"))?;
    let (tree, _rows) = rig.fork("c1")?;
    let at = tree.path();
    assert!(
        !at.join("target").exists(),
        "the clone leaves target behind"
    );
    assert!(matches!(
        carry_cargo(&rig.repo, at, "c1"),
        Carried::Units(_)
    ));
    let carried = at.join("target/debug");
    let names = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    };
    let units = names(&carried.join(".fingerprint"));
    assert!(
        units.iter().any(|name| name.starts_with("third-")),
        "{units:?}"
    );
    assert!(
        !units.iter().any(|name| name.starts_with("member-")),
        "{units:?}"
    );
    assert!(
        !units.iter().any(|name| name.starts_with("patched-")),
        "{units:?}"
    );
    assert!(
        !units.iter().any(|name| name.starts_with("app-")),
        "{units:?}"
    );
    let verbose = cargo(at, &["build", "--offline", "--verbose"])?;
    assert!(
        verbose.contains("Fresh third"),
        "the carried unit was reused: {verbose}"
    );
    assert!(verbose.contains("Compiling member"), "{verbose}");
    let printed = yi_tools::keyless_command(at.join("target/debug/app")).output()?;
    let printed = String::from_utf8_lossy(&printed.stdout).into_owned();
    let parent = rig.repo.to_string_lossy().into_owned();
    assert!(
        !printed.contains(&parent),
        "the child's binary names its parent: {printed}"
    );
    assert_eq!(
        printed.matches(at.to_string_lossy().as_ref()).count(),
        2,
        "{printed}"
    );
    assert_eq!(
        fingerprints(&rig.repo.join("target/debug"))?,
        before,
        "the parent's units stand"
    );
    Ok(())
}

#[test]
fn only_a_file_both_listed_and_ignored_carries() -> TestResult {
    let rig = Rig::new("include")?;
    std::fs::write(
        rig.repo.join(".worktreeinclude"),
        "# ours\n.env*\nREADME.md\ntarget\n",
    )?;
    std::fs::write(rig.repo.join(".env.local"), "KEY=1\n")?;
    std::fs::write(rig.repo.join("a.note"), "x")?;
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let ignored = [
        ".env.local".to_owned(),
        "a.note".to_owned(),
        "target/".to_owned(),
    ];
    let rows = carry_includes(&rig.repo, &into, &ignored, &[], &[]);
    assert_eq!(std::fs::read_to_string(into.join(".env.local"))?, "KEY=1\n");
    assert!(!into.join("a.note").exists(), "ignored but not listed");
    assert!(!into.join("README.md").exists(), "listed but not ignored");
    assert!(
        !into.join("target").exists(),
        "a listed directory is no file"
    );
    assert!(
        rows.iter().any(|row| row.contains("[not carried: target:")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("carried 1 ignored files")),
        "{rows:?}"
    );
    Ok(())
}

/// Dies with `!` read as a literal name: a broader line then carries what the exclusion refused.
#[test]
fn a_negated_pattern_refuses_what_a_broader_line_keeps() -> TestResult {
    let rig = Rig::new("include-negate")?;
    std::fs::write(
        rig.repo.join(".worktreeinclude"),
        ".env*\n!*.production.bak\n",
    )?;
    std::fs::write(rig.repo.join(".env.local"), "KEY=1\n")?;
    std::fs::write(rig.repo.join(".env.production.bak"), "SECRET=1\n")?;
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let ignored = [".env.local".to_owned(), ".env.production.bak".to_owned()];
    let rows = carry_includes(&rig.repo, &into, &ignored, &[], &[]);
    assert_eq!(std::fs::read_to_string(into.join(".env.local"))?, "KEY=1\n");
    assert!(
        !into.join(".env.production.bak").exists(),
        "a negated line refuses it: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("carried 1 ignored files")),
        "{rows:?}"
    );
    Ok(())
}

/// Dies with `keep && !skip`: a refusal outranks a later line that keeps the file, where git's last
/// matching line decides.
#[test]
fn a_later_line_outranks_an_earlier_refusal() -> TestResult {
    let rig = Rig::new("include-order")?;
    std::fs::write(
        rig.repo.join(".worktreeinclude"),
        "!*.production.bak\n.env*\n",
    )?;
    std::fs::write(rig.repo.join(".env.local"), "KEY=1\n")?;
    std::fs::write(rig.repo.join(".env.production.bak"), "SECRET=1\n")?;
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let ignored = [".env.local".to_owned(), ".env.production.bak".to_owned()];
    let rows = carry_includes(&rig.repo, &into, &ignored, &[], &[]);
    assert_eq!(std::fs::read_to_string(into.join(".env.local"))?, "KEY=1\n");
    assert_eq!(
        std::fs::read_to_string(into.join(".env.production.bak"))?,
        "SECRET=1\n",
        "a later keep line carries it: {rows:?}"
    );
    Ok(())
}

#[test]
fn the_include_file_count_holds_at_its_limit_and_says_so_one_past() -> TestResult {
    let rig = Rig::new("include-cap")?;
    let mut ignored = Vec::new();
    for n in 0..1_024 {
        let name = format!("{n}.note");
        std::fs::write(rig.repo.join(&name), "x")?;
        ignored.push(name);
    }
    let extra = ["*.note".to_owned()];
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let rows = carry_includes(&rig.repo, &into, &ignored, &extra, &[]);
    assert!(
        !rows.iter().any(|row| row.contains("fork.include_files")),
        "{rows:?}"
    );
    std::fs::write(rig.repo.join("one-more.note"), "x")?;
    ignored.push("one-more.note".to_owned());
    let rows = carry_includes(&rig.repo, &into, &ignored, &extra, &[]);
    assert!(
        rows.iter()
            .any(|row| row.contains("fork.include_files 1024")),
        "{rows:?}"
    );
    assert!(!into.join("one-more.note").exists());
    Ok(())
}

#[test]
fn a_wall_and_a_planted_link_copy_nothing() -> TestResult {
    let rig = Rig::new("include-link")?;
    let secret = rig.root.join("ssh-key");
    std::fs::write(&secret, "PRIVATE\n")?;
    std::os::unix::fs::symlink(&secret, rig.repo.join(".env"))?;
    let dir = rig.root.join("ssh");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("id"), "PRIVATE\n")?;
    std::os::unix::fs::symlink(&dir, rig.repo.join("secret"))?;
    std::fs::create_dir_all(rig.repo.join("walled"))?;
    std::fs::write(rig.repo.join("walled/f.note"), "x")?;
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let ignored = [
        ".env".to_owned(),
        "secret/id".to_owned(),
        "walled/f.note".to_owned(),
    ];
    let extra = ["**".to_owned()];
    let denied = [rig.repo.join("walled")];
    let rows = carry_includes(&rig.repo, &into, &ignored, &extra, &denied);
    assert!(!into.join(".env").exists(), "a link at the leaf");
    assert!(!into.join("secret").exists(), "a link on the way");
    assert!(!into.join("walled").exists(), "a wall still holds");
    assert_eq!(
        rows.iter()
            .filter(|row| row.contains("[not carried:"))
            .count(),
        3,
        "{rows:?}"
    );
    Ok(())
}

#[test]
fn a_bad_include_list_refuses_by_size_and_cuts_by_count() {
    assert!(include_list(&vec![b' '; 64 << 10]).is_ok());
    assert!(include_list(&vec![b' '; (64 << 10) + 1]).is_err());
    assert!(include_list(b"\xff\xfe").is_err());
    let many: String = (0..300).map(|n| format!("f{n}\n")).collect();
    let (kept, listed) = include_list(many.as_bytes()).unwrap_or_default();
    assert_eq!((kept.len(), listed), (256, 300));
}

/// Dies with the list dropped or cut in silence: the person listed `.env`, the child has none, and
/// nothing in the spawn's reply says why.
#[test]
fn every_way_a_listed_pattern_is_lost_is_named() -> TestResult {
    let rig = Rig::new("include-loud")?;
    std::fs::write(rig.repo.join("kept.note"), "x")?;
    let ignored = vec!["kept.note".to_owned()];
    let into = rig.root.join("into");
    std::fs::create_dir_all(&into)?;
    let carry = |list: Vec<u8>| -> Result<Vec<String>, std::io::Error> {
        std::fs::write(rig.repo.join(".worktreeinclude"), list)?;
        Ok(carry_includes(&rig.repo, &into, &ignored, &[], &[]))
    };

    let at: String = (0..256).map(|n| format!("*.note{n}\n")).collect();
    let rows = carry(at.clone().into_bytes())?;
    assert!(!rows.concat().contains("fork.include_patterns"), "{rows:?}");
    let past = format!("{at}*.one-more\n");
    let rows = carry(past.into_bytes())?;
    assert!(rows.concat().contains("256 of 257 patterns"), "{rows:?}");
    assert!(
        rows.concat().contains("fork.include_patterns 256"),
        "{rows:?}"
    );

    let rows = carry(vec![b' '; (64 << 10) + 1])?.concat();
    assert!(rows.contains("not read: 65537 bytes"), "{rows}");
    assert!(rows.contains("fork.include_list 65536"), "{rows}");
    assert!(carry(b"\xff\xfe".to_vec())?.concat().contains("not UTF-8"));

    let rows = carry(b"a[\n*.note\n".to_vec())?.concat();
    assert!(rows.contains("pattern `a[` does not parse"), "{rows}");
    assert!(
        into.join("kept.note").exists(),
        "a bad line took the good one: {rows}"
    );
    Ok(())
}

#[test]
fn dep_info_reads_escaped_spaces_and_skips_env_rows() {
    let parsed = dep_info(b"t.d: a.rs /x\\ y/b.rs\n\n# env-dep:FOO=bar\nt: c\\:d.rs\n");
    let want: Vec<Vec<u8>> = vec![b"a.rs".to_vec(), b"/x y/b.rs".to_vec(), b"c:d.rs".to_vec()];
    assert_eq!(parsed, want);
    assert_eq!(
        dep_info(b"\xff\xfe: /file\xe9.rs\n"),
        vec![b"/file\xe9.rs".to_vec()]
    );
}

#[test]
fn a_lossy_status_has_no_state_and_each_row_counts_once() -> TestResult {
    assert_eq!(parent_state("!! file\u{fffd}.txt\0"), None);
    let state = parent_state(" M a\0R  new\0old\0?? u\0!! i/\0").ok_or("a clean status parses")?;
    assert_eq!(state.left_out, 3);
    assert!(state.tracked_dirt);
    assert_eq!(state.skipped, vec!["u".to_owned(), "i/".to_owned()]);
    assert_eq!(state.ignored, vec!["i/".to_owned()]);
    Ok(())
}

proptest! {
    #[test]
    fn no_bytes_panic_the_parsers(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _paths = dep_info(&bytes);
        let _list = include_list(&bytes);
        let _state = parent_state(&String::from_utf8_lossy(&bytes));
    }
}

fn first_command(tree: &Path, command: &str) -> Result<yi_tools::CommandCapture, Box<dyn Error>> {
    let context = yi_tools::ToolContext::new(tree.to_path_buf());
    let timeout = std::time::Duration::from_secs(60);
    match yi_tools::jobs::run_or_background(command, &context, None, timeout, None, None)? {
        yi_tools::jobs::Run::Finished(capture) => Ok(*capture),
        _ => Err("the command did not finish".into()),
    }
}

fn with_setup(tag: &str, script: &str) -> Result<(Rig, ChildTree), Box<dyn Error>> {
    let rig = Rig::new(tag)?;
    std::fs::create_dir_all(rig.repo.join(".yi"))?;
    std::fs::write(rig.repo.join(".yi/setup"), script)?;
    git(&rig.repo, &["add", "-A"])?;
    git(&rig.repo, &["commit", "-q", "-m", "setup"])?;
    let plan = Carry {
        first_use: true,
        ..Carry::default()
    };
    let (tree, _rows) = ChildTree::fork(&rig.pool()?, &rig.repo, "c1", &plan)?;
    Ok((rig, tree))
}

#[test]
fn a_committed_setup_runs_once_before_the_first_command() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let (_rig, tree) = with_setup("setup", "printf x >> ran\n")?;
    let first = first_command(tree.path(), "touch a && cat ran")?;
    assert_eq!(first.stdout, "x", "{first:?}");
    let second = first_command(tree.path(), "touch b && cat ran")?;
    assert_eq!(second.stdout, "x", "it ran once: {second:?}");
    Ok(())
}

/// Dies with the commit check off: an edit since would be a child's code in the host's name.
#[test]
fn a_setup_edited_since_its_commit_is_skipped_by_name() -> TestResult {
    let (_rig, tree) = with_setup("setup-edit", "printf x >> ran\n")?;
    std::fs::write(tree.path().join(".yi/setup"), "printf evil >> ran\n")?;
    let first = first_command(tree.path(), "touch a")?;
    assert!(
        first
            .rows
            .iter()
            .any(|row| row == "[.yi/setup skipped: it differs from HEAD]"),
        "{first:?}"
    );
    assert!(!tree.path().join("ran").exists());
    Ok(())
}

#[test]
fn a_failed_setup_is_a_row_and_the_command_still_runs() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let (_rig, tree) = with_setup("setup-red", "echo broke >&2; exit 3\n")?;
    let first = first_command(tree.path(), "touch a")?;
    assert_eq!(first.exit_code, Some(0), "{first:?}");
    assert!(
        first
            .rows
            .iter()
            .any(|row| row.contains("[.yi/setup failed")),
        "{first:?}"
    );
    Ok(())
}

#[test]
fn the_first_writing_command_carries_units_and_says_so() -> TestResult {
    let rig = Rig::new("first-carry")?;
    std::fs::write(rig.repo.join("Cargo.lock"), "")?;
    git(&rig.repo, &["add", "-A"])?;
    git(&rig.repo, &["commit", "-q", "-m", "lock"])?;
    unit(
        &rig.repo.join("target/debug"),
        "third",
        "third",
        HASH_A,
        b"x: /elsewhere/a.rs\n",
    )?;
    let plan = Carry {
        first_use: true,
        ..Carry::default()
    };
    let (tree, _rows) = ChildTree::fork(&rig.pool()?, &rig.repo, "c1", &plan)?;
    assert!(!tree.path().join("target").exists());
    let first = first_command(tree.path(), "touch a")?;
    assert!(
        first
            .rows
            .iter()
            .any(|row| row.contains("third-party cargo units")),
        "{first:?}"
    );
    assert!(
        tree.path()
            .join(format!("target/debug/deps/libthird-{HASH_A}.rlib"))
            .is_file()
    );
    Ok(())
}
