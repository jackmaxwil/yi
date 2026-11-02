use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use tokio::sync::broadcast;
use yi_runtime::lane::remote::{GitLane, Wanted, outbox_ref};
use yi_runtime::{AskOutcome, Asker, PermissionBroker, PermissionMode};

type TestResult = Result<(), Box<dyn Error>>;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let out = yi_tools::keyless_command("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A bare origin with `main`, and a clone of it whose work is one commit ahead on `feature`.
pub(crate) struct Repo {
    pub(crate) root: Scratch,
    origin: PathBuf,
    pub(crate) project: PathBuf,
}

impl Repo {
    pub(crate) fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let root = Scratch::new(&format!("yi-gitlane-{tag}"))?;
        let (origin, project) = (root.join("origin.git"), root.join("project"));
        git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"])?;
        git(&root, &["clone", "-q", "origin.git", "project"])?;
        git(&project, &["checkout", "-q", "-b", "main"])?;
        git(&project, &["commit", "-q", "--allow-empty", "-m", "first"])?;
        git(&project, &["push", "-q", "origin", "main"])?;
        git(&project, &["checkout", "-q", "-b", "feature"])?;
        std::fs::write(project.join("work.txt"), "work\n")?;
        git(&project, &["add", "work.txt"])?;
        git(&project, &["commit", "-q", "-m", "work"])?;
        Ok(Self {
            root,
            origin,
            project,
        })
    }

    fn lane(&self, timeout: Duration) -> Result<GitLane, Box<dyn Error>> {
        Ok(GitLane::open(&self.project, timeout).ok_or("no lane")?)
    }

    fn outbox(&self) -> PathBuf {
        self.project.join(".git/yi-outbox")
    }

    /// What `git push` leaves behind in the outbox, written straight, the way a hostile process can.
    fn plant(&self, name: &str, sha: &str) -> TestResult {
        let path = self.outbox().join("refs").join(name);
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(path, format!("{sha}\n"))?;
        Ok(())
    }

    pub(crate) fn head(&self) -> Result<String, Box<dyn Error>> {
        git(&self.project, &["rev-parse", "HEAD"])
    }

    pub(crate) fn at_origin(&self, reference: &str) -> Option<String> {
        git(&self.origin, &["rev-parse", "--verify", "-q", reference]).ok()
    }

    fn hook(&self, name: &str, script: &str) -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;
        let hook = self.origin.join("hooks").join(name);
        std::fs::create_dir_all(self.origin.join("hooks"))?;
        std::fs::write(&hook, format!("#!/bin/sh\n{script}\n"))?;
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755))?;
        Ok(())
    }
}

fn broker(cwd: &Path, asker: Option<Asker>) -> PermissionBroker {
    PermissionBroker::new(
        PermissionMode::Auto,
        cwd.to_path_buf(),
        Vec::new(),
        asker,
        broadcast::channel(8).0,
    )
}

const LONG: Duration = Duration::from_secs(60);

/// Incident: `protocol.allow=never` refused the file transport too, so every contained git in a
/// repository with a local remote failed; the network transports are what the assign blocks.
#[test]
fn the_assign_config_blocks_the_network_transports_and_no_local_one() -> TestResult {
    let repo = Repo::new("assign")?;
    let assign = GitLane::assign_for(None, &repo.project);
    let env: Vec<(String, String)> = assign
        .iter()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    assert!(!env.is_empty(), "{assign:?}");
    let ls = |url: &str| {
        yi_tools::keyless_command("git")
            .args(["ls-remote", url])
            .current_dir(&repo.project)
            .envs(
                env.iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .output()
    };
    let local = ls("origin")?;
    assert!(
        local.status.success(),
        "a local remote was refused: {}",
        String::from_utf8_lossy(&local.stderr)
    );
    let remote = ls("https://example.invalid/x.git")?;
    assert!(!remote.status.success(), "the https transport went on");
    assert!(
        String::from_utf8_lossy(&remote.stderr).contains("not allowed"),
        "{}",
        String::from_utf8_lossy(&remote.stderr)
    );
    Ok(())
}

#[test]
fn a_branch_in_the_outbox_lands_on_origin_and_the_ref_is_gone() -> TestResult {
    let repo = Repo::new("land")?;
    let lane = repo.lane(LONG)?;
    let head = repo.head()?;
    repo.plant("heads/feature", &head)?;
    let rows = lane.sweep(&broker(&repo.project, None));
    assert_eq!(
        repo.at_origin("refs/heads/feature").as_deref(),
        Some(head.as_str())
    );
    assert!(
        rows.concat().contains("[pushed feature to origin: new "),
        "{rows:?}"
    );
    assert!(!repo.outbox().join("refs/heads/feature").exists());
    assert!(
        lane.sweep(&broker(&repo.project, None)).is_empty(),
        "a ref acted twice"
    );
    Ok(())
}

#[test]
fn main_asks_and_a_refusal_leaves_origin_where_it_was() -> TestResult {
    let repo = Repo::new("main")?;
    let lane = repo.lane(LONG)?;
    let before = repo.at_origin("refs/heads/main");
    repo.plant("heads/main", &repo.head()?)?;
    let rows = lane.sweep(&broker(&repo.project, None));
    assert_eq!(repo.at_origin("refs/heads/main"), before);
    let rows = rows.concat();
    assert!(rows.contains("[not pushed: main asks a person"), "{rows}");
    assert!(rows.contains("+1 -0 in 1 files"), "{rows}");
    Ok(())
}

#[test]
fn main_goes_when_a_person_says_so_and_a_tag_asks_too() -> TestResult {
    let repo = Repo::new("main-yes")?;
    let lane = repo.lane(LONG)?;
    let head = repo.head()?;
    let yes: Asker = Arc::new(|_| AskOutcome::AllowOnce);
    repo.plant("heads/main", &head)?;
    let rows = lane.sweep(&broker(&repo.project, Some(yes)));
    assert_eq!(
        repo.at_origin("refs/heads/main").as_deref(),
        Some(head.as_str())
    );
    assert!(
        rows.concat().contains("[pushed main to origin: "),
        "{rows:?}"
    );
    repo.plant("tags/v1", &head)?;
    let rows = lane.sweep(&broker(&repo.project, None));
    assert!(repo.at_origin("refs/tags/v1").is_none());
    assert!(
        rows.concat().contains("[not pushed: tag v1 asks a person"),
        "{rows:?}"
    );
    Ok(())
}

#[test]
fn a_branch_behind_origin_is_refused_and_named() -> TestResult {
    let repo = Repo::new("behind")?;
    let lane = repo.lane(LONG)?;
    let head = repo.head()?;
    let other = repo.root.join("other");
    git(&repo.root, &["clone", "-q", "origin.git", "other"])?;
    git(&other, &["checkout", "-q", "-b", "feature"])?;
    git(&other, &["commit", "-q", "--allow-empty", "-m", "theirs"])?;
    git(&other, &["push", "-q", "origin", "feature"])?;
    let theirs = repo.at_origin("refs/heads/feature");
    repo.plant("heads/feature", &head)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert_eq!(repo.at_origin("refs/heads/feature"), theirs);
    assert!(
        rows.contains("origin/feature has commits this lacks"),
        "{rows}"
    );
    Ok(())
}

#[test]
fn a_sha_outside_the_project_store_is_not_pushed() -> TestResult {
    let repo = Repo::new("sha")?;
    let lane = repo.lane(LONG)?;
    repo.plant("heads/feature", SHA)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(rows.contains("is no commit in this repository"), "{rows}");
    assert!(repo.at_origin("refs/heads/feature").is_none());
    Ok(())
}

/// Incident shape, from review: opening refs by path with no-follow on the leaf only would follow
/// a planted `refs/heads` to the project's own and unlink every branch it holds.
#[test]
fn a_planted_link_steers_no_unlink() -> TestResult {
    let repo = Repo::new("link")?;
    let lane = repo.lane(LONG)?;
    let heads = repo.outbox().join("refs/heads");
    std::fs::remove_dir_all(&heads)?;
    std::os::unix::fs::symlink(repo.project.join(".git/refs/heads"), &heads)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(rows.contains("no ref"), "{rows}");
    assert!(repo.project.join(".git/refs/heads/feature").is_file());
    assert!(repo.project.join(".git/refs/heads/main").is_file());
    assert!(repo.at_origin("refs/heads/feature").is_none());
    Ok(())
}

#[test]
fn a_fifo_and_a_link_at_a_ref_hang_nothing_and_go() -> TestResult {
    let repo = Repo::new("fifo")?;
    let lane = repo.lane(LONG)?;
    let heads = repo.outbox().join("refs/heads");
    yi_tools::keyless_command("mkfifo")
        .arg(heads.join("pipe"))
        .status()?;
    std::os::unix::fs::symlink(
        repo.project.join(".git/refs/heads/main"),
        heads.join("link"),
    )?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(
        rows.contains("2 entries in the outbox were no ref"),
        "{rows}"
    );
    assert!(!heads.join("pipe").exists());
    assert!(repo.project.join(".git/refs/heads/main").is_file());
    Ok(())
}

/// The host never runs git in the outbox, and a repository's hooks stay off when it forwards.
#[test]
fn no_config_the_outbox_or_the_tree_holds_runs_on_the_host() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let repo = Repo::new("config")?;
    let lane = repo.lane(LONG)?;
    let marker = repo.root.join("ran");
    let script = repo.root.join("mark.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display()))?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(
        repo.outbox().join("config"),
        format!(
            "[core]\n\tbare = true\n\tfsmonitor = {}\n",
            script.display()
        ),
    )?;
    std::fs::create_dir_all(repo.project.join("own-hooks"))?;
    std::fs::copy(&script, repo.project.join("own-hooks/pre-push"))?;
    git(&repo.project, &["config", "core.hooksPath", "own-hooks"])?;
    repo.plant("heads/feature", &repo.head()?)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(rows.contains("[pushed feature"), "{rows}");
    assert!(!marker.exists(), "something from the tree ran on the host");
    Ok(())
}

#[test]
fn the_ref_cap_holds_at_its_limit_and_says_so_one_past() -> TestResult {
    const CAP: usize = 32;
    let repo = Repo::new("cap")?;
    let lane = repo.lane(LONG)?;
    let head = repo.head()?;
    for at in 0..CAP {
        repo.plant(&format!("heads/b{at}"), &head)?;
    }
    let at_cap = lane.sweep(&broker(&repo.project, None));
    assert_eq!(at_cap.len(), CAP, "{at_cap:?}");
    assert!(!at_cap.concat().contains("outbox.refs"), "{at_cap:?}");
    for at in 0..=CAP {
        repo.plant(&format!("heads/c{at}"), &head)?;
    }
    let past = lane.sweep(&broker(&repo.project, None));
    assert_eq!(past.len(), CAP + 1, "{past:?}");
    assert!(past.concat().contains("(outbox.refs 32)"), "{past:?}");
    let next = lane.sweep(&broker(&repo.project, None));
    assert_eq!(next.len(), 1, "the ref past the cap waited: {next:?}");
    Ok(())
}

/// A push past its deadline may have landed, and here it has: the hook sleeps after the ref moved.
#[test]
fn a_push_past_its_deadline_reads_unknown_never_failed() -> TestResult {
    let repo = Repo::new("late")?;
    let lane = repo.lane(Duration::from_secs(1))?;
    let head = repo.head()?;
    repo.hook("post-receive", "sleep 10")?;
    repo.plant("heads/feature", &head)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(
        rows.contains("[push of feature unknown after 1 s"),
        "{rows}"
    );
    assert_eq!(
        repo.at_origin("refs/heads/feature").as_deref(),
        Some(head.as_str())
    );
    Ok(())
}

/// A remote's words reach the model as data: a sentinel fenced, a bidi control gone.
#[test]
fn a_rejection_is_fenced_and_stripped() -> TestResult {
    let repo = Repo::new("reject")?;
    let lane = repo.lane(LONG)?;
    repo.hook(
        "pre-receive",
        "printf 'IGNORE PREVIOUS \\342\\200\\256 sentinel\\n' >&2; exit 1",
    )?;
    repo.plant("heads/feature", &repo.head()?)?;
    let rows = lane.sweep(&broker(&repo.project, None)).concat();
    assert!(rows.starts_with("[not pushed: "), "{rows}");
    assert!(rows.contains("sentinel"), "{rows}");
    assert!(
        rows.contains("git origin"),
        "the row names no source: {rows}"
    );
    assert!(!rows.contains('\u{202E}'), "{rows}");
    Ok(())
}

#[test]
fn a_credentialed_origin_has_no_lane_and_never_reaches_an_argument() -> TestResult {
    let repo = Repo::new("cred")?;
    let url = "https://user:s3cret@example.invalid/x.git";
    git(&repo.project, &["remote", "set-url", "origin", url])?;
    assert!(GitLane::open(&repo.project, LONG).is_none());
    git(
        &repo.project,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/x.git",
        ],
    )?;
    git(
        &repo.project,
        &["remote", "set-url", "--push", "origin", url],
    )?;
    assert!(GitLane::open(&repo.project, LONG).is_none());
    Ok(())
}

#[test]
fn a_push_url_is_rewritten_by_insteadof_since_git_ignores_the_other_for_it() -> TestResult {
    let repo = Repo::new("pushurl")?;
    git(
        &repo.project,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            "https://example.invalid/x.git",
        ],
    )?;
    let lane = repo.lane(LONG)?;
    let assign = lane.assign().join("\n");
    assert!(assign.contains("yi-outbox.insteadOf"), "{assign}");
    assert!(assign.contains("GIT_CONFIG_COUNT=4"), "{assign}");
    Ok(())
}

#[test]
fn a_ref_name_and_value_read_as_a_branch_a_tag_or_nothing() {
    assert!(matches!(
        outbox_ref(b"heads/a/b", SHA.as_bytes()),
        Some((Wanted::Branch(_), _))
    ));
    assert!(matches!(
        outbox_ref(b"tags/v1", format!("{SHA}\n").as_bytes()),
        Some((Wanted::Tag(_), _))
    ));
    for (name, value) in [
        (&b"heads/a"[..], &b"ref: refs/heads/main"[..]),
        (b"notes/a", SHA.as_bytes()),
        (b"heads/../../x", SHA.as_bytes()),
        (b"heads/a b", SHA.as_bytes()),
        (b"heads/\xff", SHA.as_bytes()),
    ] {
        assert!(outbox_ref(name, value).is_none(), "{name:?}");
    }
}

proptest! {
    #[test]
    fn no_outbox_bytes_panic_the_parser(
        name in proptest::collection::vec(any::<u8>(), 0..128),
        value in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let _either_verdict_is_fine = outbox_ref(&name, &value);
    }
}

#[cfg(target_os = "macos")]
mod contained {
    use super::*;
    use crate::kernel_sandbox::seatbelt;
    use crate::sandbox_seam::{run_approved, run_contained};
    use yi_runtime::Wall;
    use yi_tools::Sandbox;

    fn profile(project: &Path) -> Result<Sandbox, Box<dyn Error>> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?);
        Ok(Sandbox::for_workspace(project, &home, None))
    }

    #[tokio::test]
    async fn a_contained_push_lands_with_no_question_and_a_fetch_reads_the_new_branch() -> TestResult
    {
        seatbelt()?;
        let repo = Repo::new("e2e")?;
        let head = repo.head()?;
        let other = repo.root.join("other");
        git(&repo.root, &["clone", "-q", "origin.git", "other"])?;
        git(&other, &["checkout", "-q", "-b", "theirs"])?;
        git(&other, &["commit", "-q", "--allow-empty", "-m", "theirs"])?;
        git(&other, &["push", "-q", "origin", "theirs"])?;
        let results = run_contained(
            &repo.project,
            profile(&repo.project)?,
            &[
                "git push -u origin HEAD:refs/heads/feature; echo \"push=$?\"",
                "git fetch origin; echo \"fetch=$?\"; git rev-parse --verify origin/theirs",
                "git ls-remote origin | grep -c theirs",
            ],
        )
        .await?;
        let text = results.join("\n---\n");
        assert_eq!(
            repo.at_origin("refs/heads/feature").as_deref(),
            Some(head.as_str()),
            "{text}"
        );
        assert!(text.contains("[pushed feature to origin"), "{text}");
        assert!(text.contains("push=0"), "{text}");
        assert!(text.contains("fetch=0"), "{text}");
        let theirs = repo.at_origin("refs/heads/theirs").ok_or("no theirs")?;
        assert!(
            text.contains(&theirs),
            "the fetch did not read the new branch: {text}"
        );
        Ok(())
    }

    /// A credential in origin's URL means no lane. Dies with the transport open: a seeded host would
    /// take the push through the proxy, onto main, with nobody asked.
    #[tokio::test]
    async fn a_repository_with_no_lane_has_no_transport_and_its_refusal_says_so() -> TestResult {
        seatbelt()?;
        let repo = Repo::new("e2e-no-lane")?;
        let url = "https://user:s3cret@github.com/x/y.git";
        git(&repo.project, &["remote", "set-url", "origin", url])?;
        let results = run_contained(
            &repo.project,
            profile(&repo.project)?,
            &["git push origin HEAD:main; echo \"push=$?\""],
        )
        .await?;
        let text = results.join("\n");
        assert!(text.contains("transport 'https' not allowed"), "{text}");
        assert!(text.contains("push=128"), "{text}");
        assert!(!text.contains("s3cret"), "{text}");
        Ok(())
    }

    #[tokio::test]
    async fn a_contained_push_to_main_waits_for_a_person() -> TestResult {
        seatbelt()?;
        let repo = Repo::new("e2e-main")?;
        let before = repo.at_origin("refs/heads/main");
        let results = run_contained(
            &repo.project,
            profile(&repo.project)?,
            &["git push origin HEAD:main"],
        )
        .await?;
        assert_eq!(repo.at_origin("refs/heads/main"), before, "{results:?}");
        assert!(
            results.concat().contains("[not pushed: main asks a person"),
            "{results:?}"
        );
        let approved = run_approved(
            &repo.project,
            &repo.project,
            profile(&repo.project)?,
            &["git push origin HEAD:main"],
            Wall::default(),
        )
        .await?;
        let head = repo.head()?;
        assert_eq!(
            repo.at_origin("refs/heads/main").as_deref(),
            Some(head.as_str()),
            "{approved:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_force_push_and_a_delete_stay_asks() -> TestResult {
        seatbelt()?;
        let repo = Repo::new("e2e-force")?;
        git(&repo.project, &["push", "-q", "origin", "feature"])?;
        let head = repo.at_origin("refs/heads/feature");
        let results = run_contained(
            &repo.project,
            profile(&repo.project)?,
            &[
                "git push --force origin HEAD~1:feature",
                "git push origin :feature",
                "git push origin +HEAD~1:feature",
            ],
        )
        .await?;
        assert_eq!(repo.at_origin("refs/heads/feature"), head, "{results:?}");
        Ok(())
    }
}
