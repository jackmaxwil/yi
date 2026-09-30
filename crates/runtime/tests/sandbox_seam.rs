#![cfg(target_os = "macos")]

use crate::kernel_sandbox::uncovered;
use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, AskOutcome, Asker, ConfigRule, ConfigRuleAction, PermissionBroker,
    PermissionMode, ProviderStream, SessionConfig, Wall, builtin_tools,
};
use yi_tools::Sandbox;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

pub(crate) fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

pub(crate) fn bash_call(id: &str, command: &str) -> AgentMessage {
    let mut arguments = Map::new();
    arguments.insert("command".to_owned(), json!(command));
    faux_assistant_message(
        vec![faux_tool_call(id, "bash", arguments)],
        StopReason::ToolUse,
    )
}

pub(crate) fn results(session: &AgentSession) -> Vec<String> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect()
}

/// One turn of queued bash calls under `sandbox`, with no asker: a question reads as a denial.
async fn run_contained(
    project: &Path,
    sandbox: Sandbox,
    commands: &[&str],
) -> Result<Vec<String>, Box<dyn Error>> {
    run_held(project, project, sandbox, commands, BrokerSetup::default()).await
}

/// `commands` run from `holder` under a broker built for `project`, every question approved once.
async fn run_approved(
    project: &Path,
    holder: &Path,
    sandbox: Sandbox,
    commands: &[&str],
    wall: Wall,
) -> Result<Vec<String>, Box<dyn Error>> {
    let asker: Asker = Arc::new(|_| AskOutcome::AllowOnce);
    let gate = BrokerSetup {
        asker: Some(asker),
        wall,
        ..BrokerSetup::default()
    };
    run_held(project, holder, sandbox, commands, gate).await
}

/// What the broker is built with besides the sandbox: auto mode, no rules, nobody to ask.
struct BrokerSetup {
    mode: PermissionMode,
    rules: Vec<ConfigRule>,
    asker: Option<Asker>,
    wall: Wall,
}

impl Default for BrokerSetup {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Auto,
            rules: Vec::new(),
            asker: None,
            wall: Wall::default(),
        }
    }
}

async fn run_held(
    project: &Path,
    holder: &Path,
    sandbox: Sandbox,
    commands: &[&str],
    gate: BrokerSetup,
) -> Result<Vec<String>, Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(
        commands
            .iter()
            .enumerate()
            .map(|(index, command)| bash_call(&format!("call-{index}"), command))
            .collect(),
    );
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let broker = Arc::new(
        PermissionBroker::new(
            gate.mode,
            project.to_path_buf(),
            gate.rules,
            gate.asker,
            session.events_sender(),
        )
        .with_sandbox(Some(sandbox)),
    );
    session.set_wall(gate.wall);
    session.use_tools(builtin_tools(), holder.to_path_buf(), Some(broker));
    // One turn runs every call: the loop answers each tool result with the next queued message.
    session.prompt("do the thing")?;
    session.wait_idle().await;
    let results = results(&session);
    assert_eq!(results.len(), commands.len(), "{results:?}");
    Ok(results)
}

fn confined_to(project: &Path) -> Sandbox {
    Sandbox {
        writable: vec![project.to_path_buf()],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        host_owned: Vec::new(),
    }
}

/// The production profile over a scratch tree, and a directory it does not cover: tmp is
/// writable there, so a probe under it would pass vacuously.
pub(crate) fn workspace(tag: &str) -> Result<(Scratch, PathBuf, Sandbox, PathBuf), Box<dyn Error>> {
    let root = Scratch::new(tag)?;
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let probe = uncovered(&sandbox, &home).ok_or("no writable directory outside the sandbox")?;
    Ok((root, project, sandbox, probe))
}

/// The dogfood command of 2026-09-27, its two paths moved to a writable dir and the probe dir.
fn dogfood(tmp: &Path, refused: &Path) -> String {
    format!(
        "yes | head -5; echo x > {}/sandbox_probe && echo wrote-tmp; echo y > {} && echo wrote-home",
        tmp.display(),
        refused.display()
    )
}

/// The seam, end to end: the broker contains an unknown command, the adapter
/// hands the sandbox to the tool, the command runs with no prompt and cannot
/// reach past the working tree, and the same command asks the second time
/// because containment already refused it once.
#[tokio::test]
async fn an_unknown_command_runs_contained_then_asks() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let escape = home.join("escaped.txt");
    let command = format!("printf x > {}", escape.display());
    let results = run_contained(&project, confined_to(&project), &[&command, &command]).await?;
    assert!(
        !escape.exists(),
        "the contained command must not write outside the tree"
    );
    assert!(
        !results[0].contains("Permission denied"),
        "containment runs instead of asking: {}",
        results[0]
    );
    assert!(
        results[0].contains(&format!(
            "the sandbox refused writing `{}`",
            escape.display()
        )),
        "a denial explains itself: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied"),
        "the second attempt asks rather than repeating the denial: {}",
        results[1]
    );
    Ok(())
}

/// The incident: the retry appended `&& git status | wc -l`, so an exact-text memory of the
/// refusal never matched and the second attempt was contained again instead of asking.
#[tokio::test]
async fn a_refused_program_asks_even_when_the_retry_text_differs() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam-scope")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let first = format!("mkdir {}", home.join("a").display());
    let second = format!("mkdir {} && ls", home.join("b").display());
    let results = run_contained(&project, confined_to(&project), &[&first, &second]).await?;
    assert!(
        results[0].contains(&format!("writing under `{}`", home.display())),
        "the hint names what will ask: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied") && !home.join("b").exists(),
        "a different command writing beside the refused path asks: {}",
        results[1]
    );
    Ok(())
}

/// The dogfood refusal: `yes` led the command, so the hint blamed `yes`, while the sandbox had
/// refused the redirect into the home directory.
#[tokio::test]
async fn the_hint_names_the_refused_path_not_the_first_program() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-path")?;
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let refused = probe.join(format!("yidog_probe-{}", std::process::id()));
    let results = run_contained(&project, sandbox, &[&dogfood(&tmp, &refused)]).await?;
    assert!(!refused.exists(), "the home write must be refused");
    assert!(
        results[0].contains(&format!("refused writing `{}`", refused.display()))
            && !results[0].contains("`yes`"),
        "the hint names the path the sandbox refused, not the first program: {}",
        results[0]
    );
    Ok(())
}

/// Remembering `yes` made the next harmless `yes` ask while the failing write ran contained
/// again; the memory follows the refused path instead.
#[tokio::test]
async fn a_retry_of_the_refused_write_asks_and_an_unrelated_echo_does_not() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-retry")?;
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let refused = probe.join(format!("yidog_retry-{}", std::process::id()));
    let unrelated = format!("yes | head -1 && echo hi > {}/unrelated", tmp.display());
    let retry = format!("echo y > {}", refused.display());
    let results = run_contained(
        &project,
        sandbox,
        &[&dogfood(&tmp, &refused), &unrelated, &retry],
    )
    .await?;
    assert!(
        !results[1].contains("Permission denied") && tmp.join("unrelated").exists(),
        "a call writing nowhere near the refused path still runs contained: {}",
        results[1]
    );
    assert!(
        results[2].contains("Permission denied") && !refused.exists(),
        "the retry of the refused write asks instead of failing contained again: {}",
        results[2]
    );
    Ok(())
}

/// `tail` exits 0, so the refusal of `touch` before it hid behind the pipe's exit code.
#[tokio::test]
async fn a_refusal_under_a_zero_exit_pipe_is_detected() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-pipe")?;
    let refused = probe.join(format!("yidog_touch-{}", std::process::id()));
    let command = format!("touch {} | tail -1", refused.display());
    let results = run_contained(&project, sandbox, &[&command]).await?;
    assert!(!refused.exists(), "the touch must be refused");
    assert!(
        results[0].contains(&format!("refused writing `{}`", refused.display())),
        "a refusal behind a zero exit still explains itself: {}",
        results[0]
    );
    Ok(())
}

/// The spellings a model reaches for second: `$HOME`, then the path quoted inside python. Both
/// ran contained again while the hint promised a question.
#[tokio::test]
async fn a_retry_spelled_through_home_or_python_asks() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-spell")?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // Under a tmp HOME the probe is `/Users/Shared`, which no home spelling reaches.
    if home.as_deref() != Some(probe.as_path()) {
        return Ok(());
    }
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let name = format!("yidog_spell-{}", std::process::id());
    let refused = probe.join(&name);
    let via_home = format!("echo y > $HOME/{name}");
    let via_python = format!(
        "python3 -c \"open('{}','w').write('y')\"",
        refused.display()
    );
    let results = run_contained(
        &project,
        sandbox,
        &[&dogfood(&tmp, &refused), &via_home, &via_python],
    )
    .await?;
    for (retry, result) in [&via_home, &via_python].iter().zip(&results[1..]) {
        assert!(
            result.contains("Permission denied") && !refused.exists(),
            "{retry} asks instead of failing contained again: {result}"
        );
    }
    Ok(())
}

/// A refusal in the middle of long output: the reducer keeps the head and the tail, so a broker
/// reading the reduced text found nothing and the retry ran contained again.
#[tokio::test]
async fn a_refusal_deep_in_long_output_is_remembered() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-long")?;
    let refused = probe.join(format!("yidog_long-{}.log", std::process::id()));
    let noise = |from: u32, to: u32| {
        format!(
            "seq -f 'test {from}-{to} case %g passed in 0.01s, nothing to report here' {from} {to} >&2"
        )
    };
    let first = format!(
        "{}; echo y > {}; {}",
        noise(1, 150),
        refused.display(),
        noise(152, 300)
    );
    let retry = format!("printf y > {}", refused.display());
    let results = run_contained(&project, sandbox, &[&first, &retry]).await?;
    let errno = format!("{}: Operation not permitted", refused.display());
    assert!(
        !results[0].contains(&errno) && results[0].contains("refused writing"),
        "the reducer cut the refusal line, and the hint read it from the raw output: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied") && !refused.exists(),
        "the retry asks: {}",
        results[1]
    );
    Ok(())
}

/// A directory under the uncovered probe, removed on drop: a probe left behind would be
/// writable-by-accident evidence for the next run.
pub(crate) struct Probe(pub(crate) PathBuf);

impl Probe {
    pub(crate) fn new(under: &Path, tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = under.join(format!("yi-a2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// #600: approving the retry of a refused write ran the whole command with no sandbox, so a
/// second path in the same call, one nobody was asked about, was written too.
#[tokio::test]
async fn an_approved_retry_stays_contained_and_widens_by_the_refused_dir() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-widen")?;
    let (a, b) = (Probe::new(&probe, "a")?, Probe::new(&probe, "b")?);
    let (first, second) = (a.0.join("a"), b.0.join("b"));
    let refused = format!("touch {}", first.display());
    let retry = format!("touch {}; touch {}", first.display(), second.display());
    let results = run_approved(
        &project,
        &project,
        sandbox,
        &[&refused, &retry],
        Wall::default(),
    )
    .await?;
    assert!(
        first.exists(),
        "the approved retry writes the refused dir: {}",
        results[1]
    );
    assert!(
        !second.exists(),
        "a dir nobody approved stays refused inside the approved call: {}",
        results[1]
    );
    Ok(())
}

/// The demo of #600: an approved `rm` removes what it names in the tree and nothing outside.
#[tokio::test]
async fn an_approved_destructive_command_runs_contained() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-rm")?;
    let outside = Probe::new(&probe, "rm")?;
    let (victim, escape) = (project.join("victim"), outside.0.join("escape"));
    std::fs::write(&victim, "v")?;
    std::fs::write(&escape, "e")?;
    let command = format!("rm -f {} {}", victim.display(), escape.display());
    let results = run_approved(&project, &project, sandbox, &[&command], Wall::default()).await?;
    assert!(!victim.exists(), "the approved rm ran: {}", results[0]);
    assert!(
        escape.exists(),
        "the approved rm must not reach outside the tree: {}",
        results[0]
    );
    Ok(())
}

/// A juror walled off the tree could still write it through any approved bash call: the wall
/// was a pre-check on paths the command spelled, and no profile carried it.
#[tokio::test]
async fn a_walled_juror_cannot_write_the_tree_from_an_approved_bash() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, _probe) = workspace("yi-seam-wall")?;
    let victim = project.join("victim");
    std::fs::write(&victim, "v")?;
    let wall = Wall {
        deny_write: vec![project.clone()],
        ..Wall::default()
    };
    let results = run_approved(
        &project,
        &project,
        sandbox,
        &["find . -name victim -delete"],
        wall,
    )
    .await?;
    assert!(
        victim.exists(),
        "the wall holds inside the approved call: {}",
        results[0]
    );
    Ok(())
}

/// A worktree child shares its parent's broker, whose profile covered only the parent's tree,
/// so the child's contained bash could not write its own lane.
#[tokio::test]
async fn a_worktree_childs_contained_bash_writes_its_own_lane() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-lane")?;
    let lane = Probe::new(&probe, "lane")?;
    let made = lane.0.join("made");
    let command = format!("touch made && ls {}", made.display());
    let results = run_held(
        &project,
        &lane.0,
        sandbox,
        &[&command],
        BrokerSetup::default(),
    )
    .await?;
    assert!(
        made.exists(),
        "the child's contained bash writes its own lane: {}",
        results[0]
    );
    Ok(())
}

/// A `cargo` crate whose one test binds and dials 127.0.0.1, then tries to write `escape`.
fn loopback_crate(project: &Path, escape: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"loopback_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )?;
    std::fs::write(
        project.join("src/lib.rs"),
        format!(
            "#[test]\nfn binds_loopback() {{\n    let listener = std::net::TcpListener::bind(\"127.0.0.1:0\").expect(\"bind\");\n    let _client = std::net::TcpStream::connect(listener.local_addr().expect(\"addr\")).expect(\"connect\");\n    let _refused = std::fs::write({:?}, \"out\");\n}}\n",
            escape.display().to_string()
        ),
    )
}

fn allow_rule(pattern: &str) -> Result<BrokerSetup, Box<dyn Error>> {
    Ok(BrokerSetup {
        rules: vec![ConfigRule::new("bash", pattern, ConfigRuleAction::Allow)?],
        ..BrokerSetup::default()
    })
}

/// #600 stage 2b, the demo: the gate proves `cargo test` safe, so it ran with no sandbox at all.
/// Contained, its loopback listener still works and its write outside the tree does not.
#[tokio::test]
async fn an_allowed_cargo_test_binds_loopback_and_stays_contained() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-cargo")?;
    let outside = Probe::new(&probe, "cargo")?;
    let escape = outside.0.join("escape");
    loopback_crate(&project, &escape)?;
    let command = "cargo test --offline --target-dir target";
    let results = run_contained(&project, sandbox, &[command]).await?;
    assert!(
        results[0].contains("test result: ok. 1 passed"),
        "the contained test binds 127.0.0.1: {}",
        results[0]
    );
    assert!(
        !escape.exists(),
        "an allowed call runs contained: {}",
        results[0]
    );
    Ok(())
}

/// `sort` is proven read-only, yet `-o` writes: an allowed call reached past the tree because
/// only the spelling was judged.
#[tokio::test]
async fn an_allowed_command_cannot_write_outside_the_tree() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-sort")?;
    let outside = Probe::new(&probe, "sort")?;
    let escape = outside.0.join("escape");
    std::fs::write(project.join("input"), "b\na\n")?;
    let command = format!("sort -o {} input", escape.display());
    let results = run_contained(&project, sandbox, &[&command]).await?;
    assert!(
        !escape.exists(),
        "the allowed sort must not write outside the tree: {}",
        results[0]
    );
    Ok(())
}

/// A rule allows `touch` with no question; its contained run refused, the retry asks as a
/// refused contained call does, rather than failing the same way again.
#[tokio::test]
async fn a_refused_allowed_call_asks_the_next_time() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-ruled")?;
    let outside = Probe::new(&probe, "ruled")?;
    let escape = outside.0.join("escape");
    let command = format!("touch {}", escape.display());
    let gate = allow_rule("touch *")?;
    let results = run_held(&project, &project, sandbox, &[&command, &command], gate).await?;
    assert!(
        !escape.exists() && results[0].contains("refused writing"),
        "the allowed touch runs contained: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied"),
        "the retry of a refused allowed call asks: {}",
        results[1]
    );
    assert!(
        results[1].contains("Rerun with --yolo")
            && results[1].contains("CARGO_TARGET_DIR")
            && !results[1].contains("allow rule"),
        "the headless denial names what works, and a rule the call already had is not it: {}",
        results[1]
    );
    Ok(())
}

/// `--yolo` granted everything: evals and harbor run there, and nothing contains them.
#[tokio::test]
async fn yolo_still_runs_uncontained() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-yolo")?;
    let outside = Probe::new(&probe, "yolo")?;
    let escape = outside.0.join("escape");
    let command = format!("touch {}", escape.display());
    let gate = BrokerSetup {
        mode: PermissionMode::Yolo,
        ..BrokerSetup::default()
    };
    let results = run_held(&project, &project, sandbox, &[&command], gate).await?;
    assert!(escape.exists(), "yolo runs outside: {}", results[0]);
    Ok(())
}

/// The wall matched a walled read only by its absolute spelling, so a relative path or a link
/// in the tree read it through an allowed `cat`.
#[tokio::test]
async fn an_allowed_read_cannot_open_a_walled_file_by_another_spelling() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, _probe) = workspace("yi-seam-walled")?;
    let secret = project.join("secret.txt");
    std::fs::write(&secret, "walled-canary")?;
    std::os::unix::fs::symlink(&secret, project.join("alias"))?;
    let gate = BrokerSetup {
        wall: Wall {
            deny_read: vec![secret.clone()],
            ..Wall::default()
        },
        ..BrokerSetup::default()
    };
    let spellings = ["cat secret.txt", "cat ./alias"];
    let results = run_held(&project, &project, sandbox, &spellings, gate).await?;
    for (spelling, result) in spellings.iter().zip(&results) {
        assert!(
            !result.contains("walled-canary"),
            "{spelling} read the walled file: {result}"
        );
    }
    Ok(())
}

/// A credential store read through a link in the tree: the read gate judged the spelling
/// `cat key`, found no store in it, and allowed it.
#[tokio::test]
async fn an_allowed_read_cannot_open_a_credential_store_through_a_link() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam-cred")?;
    let (project, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(home.join(".ssh"))?;
    let key = home.join(".ssh/id_probe");
    std::fs::write(&key, "credential-canary")?;
    std::os::unix::fs::symlink(&key, project.join("key"))?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let results = run_contained(&project, sandbox, &["cat key"]).await?;
    assert!(
        !results[0].contains("credential-canary"),
        "the allowed cat read a credential store: {}",
        results[0]
    );
    Ok(())
}

/// The dogfood re-run: `curl` reached the network while the tool text said "no network". A call
/// that must leave the sandbox still runs, and its result says it ran outside.
#[tokio::test]
async fn an_allowed_network_command_says_it_ran_outside() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, _probe) = workspace("yi-seam-host")?;
    let results = run_held(
        &project,
        &project,
        sandbox,
        &["ssh -V"],
        allow_rule("ssh *")?,
    )
    .await?;
    assert!(
        results[0].contains("OpenSSH") && results[0].contains("outside the sandbox (network)"),
        "the network call runs and says where: {}",
        results[0]
    );
    Ok(())
}

/// An asker that answers "always" first and refuses after, counting the questions.
pub(crate) fn always_once() -> (Asker, Arc<std::sync::atomic::AtomicUsize>) {
    let asks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&asks);
    let asker: Asker =
        Arc::new(
            move |_| match counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => AskOutcome::AllowAlways(0),
                _ => AskOutcome::Reject,
            },
        );
    (asker, asks)
}

/// Owner, round 2: "Always = session pass (Recommended)". yi's own tests nest `sandbox-exec`,
/// which a contained run refuses naming no path. The pass is the exact command the question
/// promised, "this one call": the review of #933 found it kept all of `python3`, so a
/// `python3 -c` nobody was asked about ran outside too.
#[tokio::test]
async fn always_on_a_pathless_refusal_passes_that_exact_command() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, _probe) = workspace("yi-seam-pass")?;
    std::fs::write(
        project.join("nest.py"),
        "import subprocess\nsubprocess.run(['sandbox-exec', '-p', '(version 1)(allow default)', 'true'], check=True)\nprint('nested-ok')\n",
    )?;
    let (asker, asks) = always_once();
    let gate = BrokerSetup {
        asker: Some(asker),
        ..BrokerSetup::default()
    };
    let (nested, other) = ("python3 nest.py", "python3 -c \"print('rmtree' + '-ran')\"");
    let results = run_held(
        &project,
        &project,
        sandbox,
        &[nested, nested, nested, other],
        gate,
    )
    .await?;
    assert!(
        !results[0].contains("nested-ok"),
        "the contained run refuses the nested sandbox: {}",
        results[0]
    );
    assert!(
        results[2].contains("nested-ok") && results[2].contains("outside the sandbox"),
        "the kept exact command runs outside and says so: {}",
        results[2]
    );
    assert!(
        results[3].contains("Permission denied") && !results[3].contains("rmtree-ran"),
        "another python3 command asks: {}",
        results[3]
    );
    assert_eq!(asks.load(std::sync::atomic::Ordering::SeqCst), 2);
    Ok(())
}

/// Only an "always" passes a pathless refusal: a proven or configured allow asks on the retry
/// (review of #933, mutant M10).
#[tokio::test]
async fn a_pathless_refusal_of_a_configured_allow_asks_the_next_time() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, _probe) = workspace("yi-seam-pathless")?;
    let nested = "sandbox-exec -p '(version 1)(allow default)' true && echo nested-ok";
    let results = run_held(
        &project,
        &project,
        sandbox,
        &[nested, nested],
        allow_rule("sandbox-exec *")?,
    )
    .await?;
    assert!(
        results[1].contains("Permission denied") && !results[1].contains("nested-ok"),
        "the retry asks rather than leaving the sandbox: {}",
        results[1]
    );
    assert!(
        results[1].contains("Rerun with --yolo") && !results[1].contains("allow rule"),
        "the headless denial names what works: {}",
        results[1]
    );
    Ok(())
}

/// #600 stage 2b: a command Yi proves read-only is allowed and still contained where a sandbox
/// exists; the dry run says so.
#[test]
fn a_proven_command_is_allowed_and_contained() -> TestResult {
    let dir = Scratch::new("yi-seam-safe")?;
    let report = yi_runtime::gate::explain("git status && ls", PermissionMode::Auto, &dir);
    assert_eq!(report.outcome(), "allow");
    assert_eq!(report.to_json()["sandboxed"], Sandbox::available());
    let yolo = yi_runtime::gate::explain("git status && ls", PermissionMode::Yolo, &dir);
    assert_eq!(yolo.to_json()["sandboxed"], false);
    Ok(())
}
