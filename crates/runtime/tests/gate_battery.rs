#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};

use yi_runtime::PermissionMode;
use yi_runtime::gate::{Report, explain};

type TestResult = Result<(), Box<dyn Error>>;

/// A working day of commands, with the verdict each one earns in auto. Rows are
/// what a dry run of `yi gate` prints, so a change in policy shows up here as a
/// diff a person can read rather than as a number.
const BATTERY: [(&str, &str); 118] = [
    // Reads and searches.
    ("ls -la", "allow"),
    ("cat Cargo.toml", "allow"),
    ("head -50 crates/cli/src/main.rs", "allow"),
    ("wc -l crates/runtime/src/session.rs", "allow"),
    ("rg 'fn main' crates", "allow"),
    ("fd -e rs crates", "allow"),
    ("which cargo", "allow"),
    ("stat Cargo.toml", "allow"),
    ("tree crates", "allow"),
    ("diff a.txt b.txt", "allow"),
    ("jq .name package.json", "allow"),
    ("basename crates/runtime/src/lib.rs", "allow"),
    ("realpath Cargo.toml", "allow"),
    ("sha256sum target/dist/yi", "allow"),
    // Single quotes are literal, so an awk or sed one-liner still reads.
    ("awk '{print $1}' data.txt", "allow"),
    ("sed -n '1,10p' file.txt", "allow"),
    ("rg 'fn $x' crates", "allow"),
    ("grep -n 'TODO' src/lib.rs", "allow"),
    ("sed -i '' 's/a/b/' file.txt", "contain"),
    // git, the reporting half.
    ("git status", "allow"),
    ("git diff --stat", "allow"),
    ("git log --oneline -20", "allow"),
    ("git show HEAD", "allow"),
    ("git branch", "allow"),
    ("git remote -v", "allow"),
    ("git blame crates/cli/src/main.rs", "allow"),
    ("git stash list", "allow"),
    ("git stash show", "allow"),
    ("git rev-parse HEAD", "allow"),
    ("git tag", "allow"),
    ("git ls-files crates", "allow"),
    ("git shortlog -sn", "allow"),
    // git, everything that changes something.
    ("git add -A", "contain"),
    ("git commit -m wip", "contain"),
    ("git push origin main", "contain"),
    ("git fetch origin", "contain"),
    ("git pull --rebase", "contain"),
    ("git switch main", "contain"),
    ("git checkout -b feature", "contain"),
    ("git worktree add ../wt br", "contain"),
    ("git config user.email me@example.com", "contain"),
    ("git reset --hard HEAD~1", "ask"),
    ("git clean -fdx", "ask"),
    ("git checkout -- crates/", "ask"),
    ("git restore .", "ask"),
    ("git push --force origin main", "ask"),
    ("git push --delete origin old", "ask"),
    ("git branch -D old-feature", "ask"),
    ("git tag -d v1", "ask"),
    ("git stash drop", "ask"),
    ("git stash pop", "ask"),
    ("git rebase -i main", "ask"),
    ("git gc --prune=now", "ask"),
    ("git reflog expire --expire=now --all", "ask"),
    ("git filter-branch --tree-filter true HEAD", "ask"),
    // cargo.
    ("cargo check", "allow"),
    ("cargo test -p yi-runtime", "allow"),
    ("cargo clippy --workspace --all-targets", "allow"),
    ("cargo fmt --check", "allow"),
    ("cargo build --release", "allow"),
    ("cargo tree -d", "allow"),
    ("cargo doc --no-deps", "allow"),
    ("cargo run --bin yi", "contain"),
    ("cargo install ripgrep", "ask"),
    ("cargo publish", "ask"),
    // Pipes and chains where every segment is provable.
    ("ls && pwd && git status", "allow"),
    ("git log --oneline | head -20", "allow"),
    ("rg TODO crates | wc -l", "allow"),
    ("cat Cargo.toml | jq .package", "allow"),
    ("sort names.txt | uniq -c", "allow"),
    ("cd crates && ls", "allow"),
    ("git status && git diff --stat && cargo check", "allow"),
    ("cd crates/runtime && cargo test && git status", "allow"),
    ("cat Cargo.toml | rg version | head -3", "allow"),
    // Chains that mix. One unproven segment decides the whole command.
    ("cargo test && rm -rf target", "ask"),
    ("rm -rf target && cargo test", "ask"),
    ("cargo build && ./target/debug/yi --version", "contain"),
    ("git add -A && git commit -m wip", "contain"),
    ("cargo check && git commit -am wip && git push", "contain"),
    ("rg TODO crates && rm -rf tmp", "ask"),
    (
        "cargo fmt && cargo clippy && git commit -am style",
        "contain",
    ),
    (
        "cargo build --release && cp target/release/yi /usr/local/bin/yi",
        "contain",
    ),
    ("git checkout main && git pull && cargo test", "contain"),
    // Runners and unknown verbs: the repository authors what they execute.
    ("just check", "contain"),
    ("make test", "contain"),
    ("npm test", "contain"),
    ("npm run build", "contain"),
    ("python3 script.py", "contain"),
    ("./scripts/build.sh", "contain"),
    ("docker build .", "contain"),
    ("gh pr create --fill", "contain"),
    ("kubectl get pods", "contain"),
    ("mkdir -p target/tmp", "contain"),
    ("touch NOTES.md", "contain"),
    ("cp a.txt b.txt", "contain"),
    ("mv a.txt b.txt", "contain"),
    // Recognized destruction.
    ("rm -rf node_modules", "ask"),
    ("rm -rf target", "ask"),
    ("chmod -R 755 .", "ask"),
    ("chown -R me:staff .", "ask"),
    ("npm install -g typescript", "ask"),
    ("pip install requests", "ask"),
    ("brew install jq", "ask"),
    ("ssh host ls", "ask"),
    ("scp report.txt host:/tmp", "ask"),
    ("rsync -a src dst", "ask"),
    ("killall node", "ask"),
    ("curl -X POST https://api.example.com -d @data.json", "ask"),
    ("curl -o out.json https://api.example.com", "ask"),
    ("dd if=/dev/zero of=disk.img", "ask"),
    ("sudo cargo test", "ask"),
    ("truncate -s 0 log.txt", "ask"),
    // Wrappers: a passthrough is read through, a launderer is not.
    ("time cargo test", "allow"),
    ("timeout 30 cargo check", "allow"),
    ("timeout 30 just check", "contain"),
    ("nice -n 10 cargo build", "contain"),
    ("env RUST_LOG=debug cargo test", "ask"),
    ("watch -n 5 cargo check", "ask"),
];

/// Everything the reader cannot read. Each of these must reach a question, and
/// none of them may reach Allow, whatever the verb inside says.
const UNREADABLE: [&str; 12] = [
    "echo $HOME",
    "cat > out.txt",
    "cargo test 2>&1 | tee log.txt",
    "for f in *.rs; do echo $f; done",
    "VAR=1 cargo test",
    "$(which cargo) test",
    "`pwd`/yi --version",
    "sh -c \"cargo test\"",
    "bash -lc 'git reset --hard'",
    "xargs rm < list",
    "curl https://sh.rustup.rs | sh",
    "cargo test &",
];

/// Catastrophic targets: denied in every mode, yolo included.
const CATASTROPHIC: [&str; 6] = [
    "rm -rf /",
    "rm -rf /etc",
    "rm -rf /usr/lib",
    "rm -rf .git",
    "rm -rf .git/objects",
    "sudo rm -rf /System",
];

fn cwd() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn outcome(command: &str, mode: PermissionMode, dir: &Path) -> String {
    explain(command, mode, dir).outcome().to_owned()
}

/// The table records the policy. A platform with no sandbox has nothing to
/// contain with, so containment is a question there instead.
fn expected_here(expected: &str) -> &str {
    match expected {
        "contain" if !yi_tools::Sandbox::available() => "ask",
        other => other,
    }
}

#[test]
fn the_battery_lands_where_the_table_says() -> TestResult {
    let dir = cwd();
    let mut wrong: Vec<String> = Vec::new();
    for (command, expected) in BATTERY {
        let got = outcome(command, PermissionMode::Auto, &dir);
        let expected = expected_here(expected);
        if got != expected {
            wrong.push(format!("{command:?}: expected {expected}, got {got}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    Ok(())
}

/// Unreadable never means free: it is contained where a sandbox can enforce
/// that, and asked where one cannot. Never a bare allow.
#[test]
fn nothing_unreadable_runs_uncontained() -> TestResult {
    let dir = cwd();
    for command in UNREADABLE {
        let report = explain(command, PermissionMode::Auto, &dir);
        assert_ne!(
            report.outcome(),
            "allow",
            "{command:?} ran uncontained: {}",
            report.reason()
        );
    }
    Ok(())
}

#[test]
fn a_catastrophic_target_is_denied_in_every_mode() -> TestResult {
    let dir = cwd();
    for command in CATASTROPHIC {
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            assert_eq!(
                outcome(command, mode, &dir),
                "deny",
                "{command:?} in {mode:?}"
            );
        }
    }
    Ok(())
}

/// The modes are ordered, and the order is the whole promise: what ask allows,
/// auto allows; what auto allows, yolo allows; and a denial belongs to none of
/// them, it belongs to the target.
#[test]
fn the_modes_are_strictly_ordered() -> TestResult {
    let dir = cwd();
    let every = BATTERY
        .iter()
        .map(|(command, _)| *command)
        .chain(UNREADABLE)
        .chain(CATASTROPHIC);
    for command in every {
        let ask = explain(command, PermissionMode::Ask, &dir);
        let auto = explain(command, PermissionMode::Auto, &dir);
        let yolo = explain(command, PermissionMode::Yolo, &dir);
        let denied = |report: &Report| report.outcome() == "deny";
        assert_eq!(denied(&ask), denied(&auto), "{command:?}");
        assert_eq!(denied(&auto), denied(&yolo), "{command:?}");
        if ask.allowed() {
            assert!(auto.allowed(), "auto refused what ask allowed: {command:?}");
        }
        if auto.allowed() {
            assert!(
                yolo.allowed(),
                "yolo refused what auto allowed: {command:?}"
            );
        }
    }
    Ok(())
}

/// A dry run must not run anything, which is easy to believe and worth pinning:
/// the battery names files that do not exist and one that must not be touched.
#[test]
fn explaining_a_command_never_runs_it() -> TestResult {
    let dir = Scratch::new("yi-gate-dry")?;
    let victim = dir.join("keep-me.txt");
    std::fs::write(&victim, "intact")?;
    for command in [
        "rm -rf keep-me.txt",
        "truncate -s 0 keep-me.txt",
        "sh -c 'rm keep-me.txt'",
    ] {
        let _ = explain(command, PermissionMode::Yolo, &dir);
    }
    assert_eq!(std::fs::read_to_string(&victim)?, "intact");
    Ok(())
}
