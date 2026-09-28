//! Two-way traceability through the wired plan tool: a plan drafted over three user messages that
//! serves two of them names the third as possibly forgotten, and a todo citing no message that
//! resolves as one nobody asked for; both land in the plan journal as well as the tool result.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::ProviderStream;
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::trace::{TRACE_KEY, TRACE_SHOWN, notices, trace};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::plan::doc::{GoalText, Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState, Waiver};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::url::Url;

use crate::todo_mirror::{memory_store, session, wired};

type TestResult = Result<(), Box<dyn Error>>;

fn tool_text(store: &yi_session::SharedSession, call: &str) -> Result<String, Box<dyn Error>> {
    let entries = yi_session::lock_session(store).find_entries(&yi_session::EntryQuery {
        order: yi_session::EntryOrder::OldestFirst,
        ..yi_session::EntryQuery::default()
    })?;
    entries
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Message {
                message:
                    AgentMessage::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    },
                ..
            } if tool_call_id == call => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .ok_or_else(|| format!("no tool result for {call}").into())
}

/// Dies with the check unwired or disabled: no `trace:` row reaches the model, and the journal
/// record carries no flags; or with a revision repeating a flag that still stands.
#[tokio::test]
async fn a_plan_that_misses_a_message_and_invents_a_todo_is_flagged_both_ways() -> TestResult {
    let root = Scratch::new("yi-plan-trace")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let init = json!({"op": "init", "goal": "ship the parser", "todos": [
        {"label": "keep the guardrails green", "intent": ["user://1"]},
        {"label": "wire the parser"},
        {"label": "polish the docs", "intent": ["user://9"]}
    ]});
    let drop = json!({"op": "drop", "label": "keep the guardrails green"});
    let noted = || faux_assistant_message(vec![faux_text("noted")], StopReason::Stop);
    provider.queue_faux(vec![
        noted(),
        noted(),
        faux_assistant_message(
            vec![faux_tool_call(
                "c1",
                "plan",
                init.as_object().cloned().unwrap_or_default(),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("planned")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_tool_call(
                "c2",
                "plan",
                drop.as_object().cloned().unwrap_or_default(),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("dropped")], StopReason::Stop),
    ]);
    let mut session = session(Arc::clone(&provider));
    let store = memory_store();
    session.attach_store(Arc::clone(&store))?;
    wired(&mut session, &root, provider);
    for prompt in [
        "keep the guardrails green while you work",
        "also add a changelog entry for the parser",
        "now wire the parser",
    ] {
        session.prompt_message(yi_runtime::session::user_input(prompt))?;
        session.wait_idle().await;
    }

    let text = tool_text(&store, "c1")?;
    let rows: Vec<&str> = text
        .lines()
        .filter(|row| row.starts_with("trace:"))
        .collect();
    assert_eq!(
        rows,
        [
            "trace: 1 todo(s) cite no user message that resolves, so nobody asked for them: \"polish the docs\"; cite one with intent: [\"user://<n>\"]",
            "trace: 1 user message(s) no todo cites or waives, possibly forgotten: \"user://2\"; fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]",
        ],
        "{text}"
    );
    let revised = tool_text(&store, "c2")?;
    let rows: Vec<&str> = revised
        .lines()
        .filter(|row| row.starts_with("trace:"))
        .collect();
    assert_eq!(
        rows,
        [
            "trace: 1 user message(s) no todo cites or waives, possibly forgotten: \"user://1\"; fetch one to read it, then cite it in a todo's intent or waive it with waived: [{address, reason}]"
        ],
        "a revision raises what it uncovered and repeats nothing standing: {revised}"
    );
    let id = PlanId::new("ship-the-parser")?;
    let plans = PlanStore::open(root.join("plans"))?;
    let reading = plans.journal(&id).read()?;
    let opened = reading.records.first().ok_or("no journal record")?;
    assert_eq!(
        opened.record.extra.get("trace"),
        Some(&json!({"unasked": ["polish the docs"], "forgotten": ["user://2"]}))
    );
    let plan = plans.read(&id)?;
    let defaulted = plan
        .todo(&TodoLabel::new("wire the parser")?)
        .ok_or("no todo")?;
    assert_eq!(
        defaulted
            .cites
            .intent
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["user://3"],
        "a todo declared without intent cites the prompt that drafted it"
    );
    Ok(())
}

fn todo(label: &str, intent: &[&str], waived: &[&str]) -> Result<Todo, Box<dyn Error>> {
    let mut todo = Todo::pending(TodoLabel::new(label)?);
    for url in intent {
        todo.cites.intent.push(url.parse()?);
    }
    for url in waived {
        todo.cites.waived.push(Waiver {
            address: url.parse()?,
            reason: "set aside".to_owned(),
            extra: Map::new(),
        });
    }
    Ok(todo)
}

/// Dies with a waiver not counted as cover, a dropped todo still counted, an address past the
/// session's messages read as resolving, or a message a rewind left behind called forgotten.
#[test]
fn a_waiver_covers_a_message_and_a_dropped_todo_does_not() -> TestResult {
    let mut dropped = todo("old path", &["user://2"], &[])?;
    dropped.state = TodoState::Abandoned;
    let plan = Plan::opening(
        PlanId::new("covers")?,
        GoalText::new("covers")?,
        PlanTier::Root,
        vec![
            todo("serves one", &["user://1"], &["user://3"])?,
            dropped,
            todo("past the end", &["user://5"], &[])?,
        ],
    );
    let found = trace(&plan, &[true, true, true, false]);
    assert_eq!(found.unasked, [TodoLabel::new("past the end")?]);
    assert_eq!(found.forgotten, ["user://2".parse::<Url>()?]);
    Ok(())
}

fn record_with(unasked: usize) -> Result<PlanOpRecord, Box<dyn Error>> {
    let labels: Vec<String> = (1..=unasked).map(|n| format!("todo {n}")).collect();
    let mut extra = Map::new();
    extra.insert(
        TRACE_KEY.to_owned(),
        json!({"unasked": labels, "forgotten": []}),
    );
    Ok(PlanOpRecord {
        plan: PlanId::new("capped")?,
        op: "init".to_owned(),
        actor: "main".to_owned(),
        at: 0,
        todo: None,
        from: None,
        to: None,
        todos: 0,
        extra,
    })
}

/// Dies with the cap cut silently, or with the cut row written when nothing was cut.
#[test]
fn the_trace_cap_names_its_cut_only_past_the_cap() -> TestResult {
    let at = notices(&record_with(TRACE_SHOWN)?);
    assert_eq!(at.len(), 1, "{at:?}");
    let past = notices(&record_with(TRACE_SHOWN + 1)?);
    assert_eq!(
        past.last().map(String::as_str),
        Some(
            "[… 8 of 9 shown (trace cap 8); the rest are the todos whose intent cites no user message in fetch plan://capped]"
        ),
        "{past:?}"
    );
    assert!(
        past.first()
            .is_some_and(|row| row.contains("\"todo 8\"") && !row.contains("\"todo 9\"")),
        "{past:?}"
    );
    Ok(())
}

fn call(id: &str, args: serde_json::Value) -> AgentMessage {
    let args = args.as_object().cloned().unwrap_or_default();
    faux_assistant_message(vec![faux_tool_call(id, "plan", args)], StopReason::ToolUse)
}

/// Dies with the pick unread: the user's "2" leaves no answer, the unblock names no exemplar and
/// the reply's address never joins the intent; or with an unblock before any reply let through.
#[tokio::test]
async fn a_todo_asking_three_options_takes_the_users_pick_by_number() -> TestResult {
    let root = Scratch::new("yi-plan-ask")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let option = |id: &str, label: &str| json!({"id": id, "label": label, "preview": format!("hero: {label}")});
    let asked = |options: Vec<serde_json::Value>| {
        json!({"op": "block", "label": "hero style", "on": {"user": null},
            "note": "which hero?", "options": options})
    };
    let three = vec![
        option("a", "Calm"),
        option("b", "Bold"),
        option("c", "Dense"),
    ];
    provider.queue_faux(vec![
        call(
            "c1",
            json!({"op": "init", "goal": "land the page", "todos": [{"label": "hero style"}]}),
        ),
        call("c2", asked(three[..2].to_vec())),
        call("c3", asked(three)),
        call("c4", json!({"op": "unblock", "label": "hero style"})),
        faux_assistant_message(vec![faux_text("which hero?")], StopReason::Stop),
        call("c5", json!({"op": "unblock", "label": "hero style"})),
        faux_assistant_message(vec![faux_text("bold it is")], StopReason::Stop),
    ]);
    let mut session = session(Arc::clone(&provider));
    let store = memory_store();
    session.attach_store(Arc::clone(&store))?;
    wired(&mut session, &root, provider);
    for prompt in ["draft the landing page hero", "2"] {
        session.prompt_message(yi_runtime::session::user_input(prompt))?;
        session.wait_idle().await;
    }

    let two = tool_text(&store, "c2")?;
    assert!(two.contains("offers 3 to 5 options, not 2"), "{two}");
    let block = tool_text(&store, "c3")?;
    assert!(
        block.contains("options 1. Calm · 2. Bold · 3. Dense;")
            && block.contains("unattended it stays blocked: nothing picks for the user"),
        "{block}"
    );
    let early = tool_text(&store, "c4")?;
    assert!(
        early.contains("with none since it asked, end your turn"),
        "an unblock with no reply since the ask is refused: {early}"
    );
    let picked = tool_text(&store, "c5")?;
    assert!(
        picked.contains("picked b Bold by user://2, the exemplar; rejected a, c"),
        "{picked}"
    );
    let plan = PlanStore::open(root.join("plans"))?.read(&PlanId::new("land-the-page")?)?;
    let todo = plan.todo(&TodoLabel::new("hero style")?).ok_or("no todo")?;
    assert_eq!(todo.state, TodoState::Pending);
    let ask = todo
        .ask
        .as_ref()
        .ok_or("the ask is kept past the unblock")?;
    let answer = ask.answer.as_ref().ok_or("no answer recorded")?;
    assert_eq!(answer.address.to_string(), "user://2");
    assert_eq!(answer.option.as_ref().map(|id| id.as_str()), Some("b"));
    assert_eq!(
        ask.options.len(),
        3,
        "the rejected options stay on the record"
    );
    let intent: Vec<String> = todo.cites.intent.iter().map(ToString::to_string).collect();
    assert_eq!(intent, ["user://1", "user://2"]);
    Ok(())
}

fn hero_block() -> serde_json::Value {
    let option = |id: &str, label: &str| json!({"id": id, "label": label});
    json!({"op": "block", "label": "hero style", "on": {"user": null}, "note": "which hero?",
        "options": [option("calm", "Calm hero"), option("serif", "Bold serif"), option("sans", "Bold sans")]})
}

/// A recorded answer: the reply's address and the option it picked, if any.
type Picked = (String, Option<String>);

struct Asking {
    root: Scratch,
    provider: Arc<ProviderStream>,
    session: yi_runtime::AgentSession,
    store: yi_session::SharedSession,
}

impl Asking {
    /// A wired session whose plan's "hero style" waits on three options, asked on `first`.
    async fn new(first: AgentMessage) -> Result<Self, Box<dyn Error>> {
        let root = Scratch::new("yi-plan-pick")?;
        let provider = Arc::new(ProviderStream::new(None, None));
        let init =
            json!({"op": "init", "goal": "land the page", "todos": [{"label": "hero style"}]});
        provider.queue_faux(vec![
            call("i", init),
            call("b", hero_block()),
            faux_assistant_message(vec![faux_text("which hero?")], StopReason::Stop),
        ]);
        let mut session = session(Arc::clone(&provider));
        let store = memory_store();
        session.attach_store(Arc::clone(&store))?;
        wired(&mut session, &root, Arc::clone(&provider));
        session.prompt_message(first)?;
        session.wait_idle().await;
        Ok(Self {
            root,
            provider,
            session,
            store,
        })
    }

    /// One turn on `prompt` whose model makes the plan call `op` as call `u`, if any.
    async fn turn(&self, prompt: AgentMessage, op: Option<serde_json::Value>) -> TestResult {
        let mut replies: Vec<AgentMessage> = op.into_iter().map(|op| call("u", op)).collect();
        replies.push(faux_assistant_message(
            vec![faux_text("ok")],
            StopReason::Stop,
        ));
        self.provider.queue_faux(replies);
        self.session.prompt_message(prompt)?;
        self.session.wait_idle().await;
        Ok(())
    }

    async fn unblock_on(&self, reply: &str) -> TestResult {
        let unblock = json!({"op": "unblock", "label": "hero style"});
        self.turn(yi_runtime::session::user_input(reply), Some(unblock))
            .await
    }

    fn todo(&self) -> Result<Todo, Box<dyn Error>> {
        let plan =
            PlanStore::open(self.root.join("plans"))?.read(&PlanId::new("land-the-page")?)?;
        Ok(plan
            .todo(&TodoLabel::new("hero style")?)
            .ok_or("no hero todo")?
            .clone())
    }

    fn answer(&self) -> Result<Option<Picked>, Box<dyn Error>> {
        let todo = self.todo()?;
        let answer = todo.ask.and_then(|ask| ask.answer);
        Ok(answer.map(|answer| {
            let option = answer.option.map(|id| id.as_str().to_owned());
            (answer.address.to_string(), option)
        }))
    }
}

fn host(text: &str) -> AgentMessage {
    AgentMessage::host_user(yi_types::message::UserContent::Text(text.to_owned()), 0)
}

/// Dies with a reply that mentions an option in passing, names two, or only shares a label's
/// first word recorded as the user's pick, the worst failure: a choice the user never made.
#[tokio::test]
async fn a_reply_picks_only_when_it_opens_with_one_option_and_names_no_other() -> TestResult {
    let cases: [(&str, Option<&str>); 15] = [
        ("2", Some("serif")),
        ("#3", Some("sans")),
        ("serif, but calmer", Some("serif")),
        ("bold serif", Some("serif")),
        ("calm.", Some("calm")),
        ("2 more things: fix the footer first", None),
        ("2 and 3", None),
        ("2, 3", None),
        ("1, though 3's colours", None),
        ("no, none of these", None),
        ("maybe serif?", None),
        ("serif but calmer", None),
        ("Bold", None),
        ("Ça, 2", None),
        ("２", None),
    ];
    let mut wrong = Vec::new();
    for (reply, want) in cases {
        let asking = Asking::new(yi_runtime::session::user_input("draft the hero")).await?;
        asking.unblock_on(reply).await?;
        let got = asking.answer()?.ok_or("every reply is recorded")?;
        if got != ("user://2".to_owned(), want.map(str::to_owned)) {
            wrong.push(format!("{reply:?} -> {got:?}, want {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// Dies with an older pick outliving the user's newer word: "2" then "none of these" is not 2.
#[tokio::test]
async fn the_newest_reply_decides_so_a_retraction_is_not_a_pick() -> TestResult {
    let asking = Asking::new(yi_runtime::session::user_input("draft the hero")).await?;
    asking
        .turn(yi_runtime::session::user_input("2"), None)
        .await?;
    asking
        .unblock_on("actually, none of these: make it green")
        .await?;
    assert_eq!(asking.answer()?, Some(("user://3".to_owned(), None)));
    Ok(())
}

/// Dies with a pick the user rewound away counted as their answer when a wake unblocks.
#[tokio::test]
async fn a_pick_on_a_rewound_branch_does_not_answer() -> TestResult {
    let asking = Asking::new(yi_runtime::session::user_input("draft the hero")).await?;
    asking
        .turn(yi_runtime::session::user_input("2"), None)
        .await?;
    let typed = yi_session::lock_session(&asking.store).find_entries(&yi_session::EntryQuery {
        order: yi_session::EntryOrder::NewestFirst,
        ..yi_session::EntryQuery::default()
    })?;
    let two = typed
        .iter()
        .find(|entry| {
            matches!(entry, Entry::Message { message: AgentMessage::User { content: yi_types::message::UserContent::Text(text), .. }, .. } if text == "2")
        })
        .ok_or("no typed 2")?;
    yi_runtime::rewind::rewind_to(&asking.session, two.id())?;
    let unblock = json!({"op": "unblock", "label": "hero style"});
    asking.turn(host("[wake]"), Some(unblock)).await?;
    let refused = tool_text(&asking.store, "u")?;
    assert!(refused.contains("waits on the user's pick"), "{refused}");
    assert_eq!(asking.answer()?, None);
    Ok(())
}

/// Dies with a child stuck forever: no one types into a child's session, so a refusal that waits
/// for a typed reply never lifts; its parent's mail answers, unrecorded.
#[tokio::test]
async fn a_session_no_user_types_into_unblocks_its_ask_unrecorded() -> TestResult {
    let asking = Asking::new(host("draft the hero")).await?;
    let unblock = json!({"op": "unblock", "label": "hero style"});
    asking.turn(host("parent: serif"), Some(unblock)).await?;
    let text = tool_text(&asking.store, "u")?;
    assert_eq!(asking.todo()?.state, TodoState::Pending, "{text}");
    assert_eq!(asking.answer()?, None);
    Ok(())
}

/// Dies with `set` rewriting the asked todo to pending: the unblock's refusal routed around.
#[tokio::test]
async fn a_set_cannot_move_an_unanswered_ask_off_blocked() -> TestResult {
    let asking = Asking::new(yi_runtime::session::user_input("draft the hero")).await?;
    let set = json!({"op": "set", "list": "- [ ] hero style\n"});
    asking
        .turn(yi_runtime::session::user_input("go on"), Some(set))
        .await?;
    let refused = tool_text(&asking.store, "u")?;
    assert!(refused.contains("waits on the user's pick"), "{refused}");
    assert!(
        matches!(asking.todo()?.state, TodoState::Blocked { .. }),
        "{refused}"
    );
    Ok(())
}

/// Dies with a declaring op reading the owner's messages twice (its citing default, then its
/// trace) or a non-declaring op reading them at all: each read walks the whole session.
#[test]
fn a_plan_op_reads_the_owners_messages_at_most_once() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use yi_runtime::plan::ops::{Actor, Op, OpRequest, PlanEngine, TodoSpec};

    let root = Scratch::new("yi-plan-trace-reads")?;
    let store = memory_store();
    yi_session::lock_session(&store)
        .append_message("main", yi_runtime::session::user_input("ship the parser"))?;
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reads);
    let engine = PlanEngine::new(
        PlanStore::open(root.join("plans"))?,
        Arc::new(crate::plan_e2e::NoChildren),
    )
    .with_owner_words(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Some(Arc::clone(&store))
    }));
    let spec = |label: &str| -> Result<TodoSpec, Box<dyn Error>> {
        Ok(serde_json::from_value(json!({"label": label}))?)
    };
    let ops = [
        Op::Init {
            goal: GoalText::new("ship the parser")?,
            todos: vec![spec("wire the parser")?],
        },
        Op::Start {
            label: TodoLabel::new("wire the parser")?,
        },
        Op::Append {
            todos: vec![spec("polish the docs")?],
        },
    ];
    let mut seen = Vec::new();
    for op in ops {
        engine.apply(OpRequest {
            plan: None,
            actor: Actor::Owner,
            op,
            request_id: None,
            expected_revision: None,
        })?;
        seen.push(reads.load(Ordering::SeqCst));
    }
    assert_eq!(
        seen,
        [1, 1, 2],
        "cumulative reads after init, start, append"
    );
    Ok(())
}
