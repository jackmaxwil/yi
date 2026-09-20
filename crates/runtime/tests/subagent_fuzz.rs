//! Random child lifecycles over the real [`yi_runtime::SubagentHost`]: a client that knows
//! only the parent bus and the roster folds both into cards, and no card may say `Running`
//! once the host holds no record for it. A failing property shrinks to the shortest sequence.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::{Just, Strategy, prop, prop_oneof};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use serde_json::{Map, Value};
use tokio::sync::broadcast::error::TryRecvError;
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::event::AgentEvent;
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};
use yi_types::subagent::ChildStatus;

/// Invariant: the lane rides `just check`, so the budget stays a few seconds; a soak raises
/// PROPTEST_CASES instead (the stage's exit ran 10,000).
const CASES: u32 = 48;
const MAX_ACTIONS: usize = 10;

#[derive(Debug, Clone)]
enum Action {
    Spawn,
    /// Lets every spawned run task be polled, so some children end on their own.
    Settle,
    Interrupt(usize),
    Delete(usize),
    Reap(usize),
    /// The client's bus receiver overflowed: everything queued is lost to it.
    Lag,
}

fn faux_model() -> Model {
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

type Bus = tokio::sync::broadcast::Receiver<AgentEvent>;

fn host(root: &Scratch) -> (Arc<SubagentHost>, Bus) {
    host_with(root, Arc::new(|| {}))
}

/// `gate` runs inside the factory, which is where a slow build blocks.
fn host_with(root: &Scratch, gate: Arc<dyn Fn() + Send + Sync>) -> (Arc<SubagentHost>, Bus) {
    let (events, bus) = tokio::sync::broadcast::channel(1024);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: root.to_path_buf(),
        cwd: root.to_path_buf(),
        home: root.join("home"),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(move |build| {
            gate();
            let provider = Arc::new(ProviderStream::new(None, None));
            provider.queue_faux(vec![faux_assistant_message(
                vec![faux_text("done")],
                StopReason::Stop,
            )]);
            Ok(AgentSession::new(
                SessionConfig {
                    system_prompt: "child".to_owned(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            ))
        }),
        notice: Arc::new(|_| {}),
        events,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_| {}),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: root.join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }));
    (host, bus)
}

/// The client: a card per child id, fed by the bus and, when asked, the roster.
#[derive(Default)]
struct Cards(HashMap<String, ChildStatus>);

impl Cards {
    fn drain(&mut self, bus: &mut tokio::sync::broadcast::Receiver<AgentEvent>) {
        loop {
            match bus.try_recv() {
                Ok(AgentEvent::ChildUpdate { update }) => {
                    self.0.insert(update.id.as_str().to_owned(), update.status);
                }
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
            }
        }
    }

    /// The roster rule a client heals a gap with: a running card the host no longer lists
    /// is gone.
    fn reconcile(&mut self, host: &SubagentHost) {
        let roster = host.children_view();
        for child in &roster {
            self.0
                .insert(child.update.id.as_str().to_owned(), child.update.status);
        }
        for (id, status) in &mut self.0 {
            let listed = roster.iter().any(|child| child.update.id.as_str() == id);
            if !listed && *status == ChildStatus::Running {
                *status = ChildStatus::Error;
            }
        }
    }

    fn orphans(&self, host: &SubagentHost) -> Vec<String> {
        let roster = host.children_view();
        self.0
            .iter()
            .filter(|(id, status)| {
                **status == ChildStatus::Running
                    && !roster.iter().any(|child| child.update.id.as_str() == *id)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

fn fail(error: impl std::fmt::Display) -> TestCaseError {
    TestCaseError::fail(error.to_string())
}

async fn run_case(actions: &[Action]) -> Result<(), TestCaseError> {
    let root = Scratch::new("yi-subagent-fuzz").map_err(fail)?;
    let (host, mut bus) = host(&root);
    let mut cards = Cards::default();
    let mut names: Vec<String> = Vec::new();
    let mut lagged = false;
    for action in actions {
        let pick = |index: &usize| names.get(index % names.len().max(1)).cloned();
        match action {
            Action::Spawn => {
                let mut kwargs = Map::new();
                let name = format!("child-{}", names.len());
                kwargs.insert("name".to_owned(), Value::String(name.clone()));
                // A refusal (the cap) is a legal answer; only an admitted child gets a card.
                if host.spawn("work".to_owned(), kwargs).is_ok() {
                    names.push(name);
                }
            }
            Action::Settle => tokio::time::sleep(Duration::from_millis(20)).await,
            Action::Interrupt(index) => {
                let _gone_is_legal = pick(index).map(|name| host.interrupt(&name));
            }
            Action::Delete(index) => {
                let _gone_is_legal = pick(index).map(|name| host.delete(&name));
            }
            Action::Reap(index) => {
                let _gone_is_legal = pick(index).map(|name| host.reap(&name));
            }
            Action::Lag => {
                lagged = true;
                bus = bus.resubscribe();
            }
        }
    }
    // Every run the host still holds ends on its own; then the bus has said all it will.
    for _ in 0..400 {
        let live = host
            .children_view()
            .iter()
            .any(|child| child.update.status == ChildStatus::Running);
        if !live {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    cards.drain(&mut bus);
    if !lagged {
        let orphans = cards.orphans(&host);
        if !orphans.is_empty() {
            return Err(fail(format!(
                "the bus alone left {orphans:?} running with no record"
            )));
        }
    }
    cards.reconcile(&host);
    let orphans = cards.orphans(&host);
    if orphans.is_empty() {
        Ok(())
    } else {
        Err(fail(format!(
            "after the roster, {orphans:?} still run with no record"
        )))
    }
}

#[test]
fn no_card_runs_without_a_record() -> Result<(), Box<dyn Error>> {
    let action = prop_oneof![
        3 => Just(Action::Spawn),
        1 => Just(Action::Settle),
        1 => (0..4_usize).prop_map(Action::Interrupt),
        2 => (0..4_usize).prop_map(Action::Delete),
        1 => (0..4_usize).prop_map(Action::Reap),
        1 => Just(Action::Lag),
    ];
    let mut config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = CASES;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    TestRunner::new(config)
        .run(&prop::collection::vec(action, 1..MAX_ACTIONS), |actions| {
            runtime.block_on(run_case(&actions))
        })
        .map_err(|error| format!("{error}"))?;
    Ok(())
}

/// Guards the roster lock: `spawn` held it across the factory, so one slow build froze
/// `states`, `list` and every client's roster tick behind it.
#[test]
fn states_answers_while_a_child_is_being_built() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-subagent-build")?;
    let (entered_tx, entered) = std::sync::mpsc::channel::<()>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    let (host, _bus) = host_with(
        &root,
        Arc::new(move || {
            let _ = entered_tx.send(());
            let _ = released.lock().map(|gate| gate.recv());
        }),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    let builder = Arc::clone(&host);
    let spawning = std::thread::spawn(move || {
        let _context = handle.enter();
        let mut kwargs = Map::new();
        kwargs.insert("name".to_owned(), Value::String("slow".to_owned()));
        builder.spawn("work".to_owned(), kwargs)
    });
    entered.recv_timeout(Duration::from_secs(10))?;
    let (answered_tx, answered) = std::sync::mpsc::channel();
    let reader = Arc::clone(&host);
    std::thread::spawn(move || answered_tx.send(reader.states().len()));
    let seen = answered.recv_timeout(Duration::from_secs(5));
    // The name is held while the build runs, so a second spawn cannot take it.
    let _context = runtime.enter();
    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String("slow".to_owned()));
    let twin = host.spawn("work".to_owned(), kwargs);
    release.send(())?;
    release.send(())?;
    assert_eq!(seen, Ok(0), "states blocked behind a build in flight");
    assert!(
        twin.is_err_and(|error| error.contains("already taken")),
        "a name under construction was handed out twice"
    );
    assert!(spawning.join().map_err(|_| "spawn panicked")?.is_ok());
    assert_eq!(
        host.states().len(),
        1,
        "the built child took its reserved slot"
    );
    Ok(())
}
