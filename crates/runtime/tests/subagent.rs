//! The host's own rules for a child: how an exit reads, and what a spawn may draw (D215).

use crate::scratch;
use crate::support;

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use yi_runtime::family::{MemberState, read_exit};
use yi_types::subagent::{ChildExit, ChildStatus, ChildUpdate, FailClass};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Dies with `read_exit` as the one reading: give any surface its own mapping and one of these
/// rows splits, which is the TUI showing `completed` for a child the model was told had failed.
#[test]
fn status_state_and_notice_derive_from_one_exit() -> TestResult {
    let provider = ChildExit::Failed {
        class: FailClass::Provider,
    };
    let rows = [
        (None, ChildStatus::Running, MemberState::Running, "running"),
        (
            Some(ChildExit::Completed),
            ChildStatus::Completed,
            MemberState::Finished,
            "finished",
        ),
        (
            Some(provider),
            ChildStatus::Error,
            MemberState::Failed,
            "failed",
        ),
        (
            Some(ChildExit::Interrupted),
            ChildStatus::Error,
            MemberState::Failed,
            "interrupted",
        ),
        (
            Some(ChildExit::Reaped),
            ChildStatus::Error,
            MemberState::Failed,
            "reaped",
        ),
        (
            Some(ChildExit::Repossessed),
            ChildStatus::Error,
            MemberState::Failed,
            "repossessed",
        ),
    ];
    for (exit, status, state, verb) in rows {
        let reading = read_exit(exit);
        assert_eq!(
            (reading.status, reading.state, reading.verb),
            (status, state, verb)
        );
    }
    // The wire keeps its three status words; the exit rides beside them and may be absent.
    let old = r#"{"id":"sub-1","name":"a","status":"error","activity":"waiting","toolUseCount":0,"tokenCount":0}"#;
    let parsed: ChildUpdate = serde_json::from_str(old)?;
    assert_eq!((parsed.status, parsed.exit), (ChildStatus::Error, None));
    assert_eq!(serde_json::to_string(&parsed)?, old);
    let typed = ChildUpdate {
        exit: Some(provider),
        ..parsed
    };
    assert!(
        serde_json::to_string(&typed)?.ends_with(r#""exit":{"kind":"failed","class":"provider"}}"#)
    );
    Ok(())
}

fn kwargs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    std::iter::once(("role", json!("root")))
        .chain(pairs.iter().cloned())
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn family(label: &str) -> Result<(scratch::Scratch, support::Family), Box<dyn std::error::Error>> {
    let root = scratch::Scratch::new(label)?;
    let store = support::memory_store(label);
    let family = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    Ok((root, family))
}

/// Dies with the refusal in `draw`: clamp instead and a child told it has ten minutes is cut
/// at two with no word why, which is the silent failure the both-numbers refusal replaces.
#[tokio::test]
async fn a_deadline_past_the_parents_bound_is_refused_with_both_numbers() -> TestResult {
    let (_root, family) = family("yi-lease-deadline")?;
    family
        .host
        .set_deadline(Instant::now().checked_add(Duration::from_secs(120)));
    let spawn = |name: &str, seconds: u64| {
        let asked = kwargs(&[("name", json!(name)), ("deadline_s", json!(seconds))]);
        family.host.spawn("work".to_owned(), asked)
    };
    let refused = spawn("greedy", 600)
        .err()
        .ok_or("a ten minute ask was admitted")?;
    assert!(refused.contains("600000 ms"), "the ask is named: {refused}");
    let bound = refused
        .split_whitespace()
        .filter_map(|word| word.parse::<u64>().ok())
        .find(|number| (80_000..=90_000).contains(number));
    assert!(bound.is_some(), "the parent's bound is named: {refused}");
    assert!(family.built.lock().map_err(|_| "poisoned")?.is_empty());

    spawn("modest", 60)?;
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("heir"))]))?;
    let built = family.built.lock().map_err(|_| "poisoned")?;
    assert_eq!(built[0].deadline, Some(Duration::from_secs(60)));
    let inherited = built[1]
        .deadline
        .ok_or("an omitted deadline inherits the bound")?;
    assert!(
        (Duration::from_secs(80)..=Duration::from_secs(90)).contains(&inherited),
        "the parent's clock less its cleanup grace: {inherited:?}"
    );
    drop(built);

    family.host.set_deadline(Some(Instant::now()));
    let expired = spawn("late", 1)
        .err()
        .ok_or("an expired parent leased time")?;
    assert!(expired.contains("has passed"), "{expired}");
    Ok(())
}

/// Dies with the reservation in `draw` and `Children::reserved`: mint instead and two children
/// each draw the whole budget, so the root's one budget is spent twice.
#[tokio::test]
async fn a_spawn_asking_past_the_parents_deadline_or_tokens_is_refused_with_both_numbers()
-> TestResult {
    let (_root, family) = family("yi-lease-tokens")?;
    family
        .host
        .set_grant(yi_runtime::Wall::default(), Some(5_000));
    let spawn = |name: &str, tokens: u64| {
        let asked = kwargs(&[("name", json!(name)), ("tokens", json!(tokens))]);
        family.host.spawn("work".to_owned(), asked)
    };
    spawn("first", 3_000)?;
    let refused = spawn("second", 3_000)
        .err()
        .ok_or("the budget was leased twice")?;
    assert!(
        refused.contains("3000") && refused.contains("2000 of its 5000"),
        "the ask and what is left are both named: {refused}"
    );
    spawn("third", 2_000)?;
    assert_eq!(
        family.built.lock().map_err(|_| "poisoned")?[1].tokens,
        Some(2_000)
    );
    let zero = spawn("fourth", 0)
        .err()
        .ok_or("a zero lease was admitted")?;
    assert!(zero.contains("positive"), "{zero}");
    Ok(())
}

/// Dies with the refusal in `Ask::from_kwargs`: admit it and a child outlives its parent with
/// nobody holding its address, budget, inbox, artifacts or deadline.
#[tokio::test]
async fn abandon_is_refused_until_a_supervisor_exists() -> TestResult {
    let (_root, family) = family("yi-lease-abandon")?;
    let spawn = |policy: Value| {
        let asked = kwargs(&[("name", json!("orphan")), ("parent_close", policy)]);
        family.host.spawn("work".to_owned(), asked)
    };
    for policy in [json!("abandon"), json!({"policy": "abandon"})] {
        let refused = spawn(policy).err().ok_or("abandon was admitted")?;
        assert!(refused.contains("abandon"), "{refused}");
    }
    assert!(family.built.lock().map_err(|_| "poisoned")?.is_empty());
    spawn(json!({"policy": "terminate", "grace_ms": 5_000}))?;
    Ok(())
}

/// Dies with `Wall::under` at spawn: drop it and a walled child hands its own child the file
/// it was walled off from, by spawning it with no wall at all.
#[tokio::test]
async fn a_child_cannot_spawn_with_a_smaller_wall_than_its_parent() -> TestResult {
    let (_root, family) = family("yi-lease-wall")?;
    let held = yi_runtime::Wall {
        deny_write: vec!["/repo/spec.md".into()],
        deny_read: vec!["/repo/secrets".into()],
        deny_url: vec!["kernel://".to_owned()],
        container: None,
    };
    family.host.set_grant(held.clone(), None);
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("bare"))]))?;
    let own = kwargs(&[("name", json!("own")), ("deny_url", json!(["plan://"]))]);
    family.host.spawn("work".to_owned(), own)?;
    let built = family.built.lock().map_err(|_| "poisoned")?;
    assert_eq!(
        built[0].wall, held,
        "no wall asked for is still the parent's wall"
    );
    assert_eq!(built[1].wall.deny_write, held.deny_write);
    assert_eq!(built[1].wall.deny_url, ["plan://", "kernel://"]);
    Ok(())
}

/// Dies with `seen` advanced by every reply: the run loop's own cursor moves the model's bare
/// one past a child it never saw, so its next bare wait blocks to the cap on a finished child.
#[tokio::test]
async fn a_cursor_carrying_wait_leaves_the_bare_cursor_alone() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let (_root, family) = family("yi-wait-cursor")?;
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let wait = |cursor: Option<u64>| {
        let mut payload = kwargs(&[("timeout_ms", json!(1_000))]);
        if let Some(cursor) = cursor {
            payload.insert("cursor".to_owned(), json!(cursor));
        }
        registry.dispatch("rlm.wait", payload)
    };
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("a"))]))?;
    assert!(family.reaches("a", "finished").await);
    let bare = wait(None).ok_or("rlm.wait")?.await?;
    let epoch = bare["cursor"].as_u64().ok_or("cursor")?;
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("b"))]))?;
    assert!(family.reaches("b", "finished").await);
    wait(Some(epoch)).ok_or("rlm.wait")?.await?;
    let started = Instant::now();
    let again = wait(None).ok_or("rlm.wait")?.await?;
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "b moved since the bare waiter last looked"
    );
    assert!(
        again["changed"]
            .as_array()
            .is_some_and(|names| names.iter().any(|name| name.as_str() == Some("b"))),
        "{again:?}"
    );
    Ok(())
}

/// Dies with the reap recorded at the finish's epoch: the waiter had read `a` finish, so the reap
/// was no news, and with `b` live the next bare wait slept to its timeout. The reap wakes it once,
/// named: a wake naming no one answered `rlm.wait(300)` in 20 ms, four times.
#[tokio::test]
async fn a_bare_wait_wakes_once_on_the_reap_of_a_child_it_saw_finish() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let root = scratch::Scratch::new("yi-wait-reap")?;
    let store = support::memory_store("yi-wait-reap");
    let family = support::family(
        root.to_path_buf(),
        std::env::temp_dir(),
        store,
        Some("sleep 3"),
    );
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let wait = || {
        let payload = kwargs(&[("timeout_ms", json!(1_000))]);
        registry.dispatch("rlm.wait", payload)
    };
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("a"))]))?;
    assert!(family.reaches("a", "finished").await);
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("b"))]))?;
    wait().ok_or("rlm.wait")?.await?;
    family.host.delete("a")?;
    let started = Instant::now();
    let again = wait().ok_or("rlm.wait")?.await?;
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "the reap is news: {again:?}"
    );
    let names = again["changed"].as_array().ok_or("changed")?;
    assert!(names.contains(&json!("a")), "{again:?}");
    let later = wait().ok_or("rlm.wait")?.await?;
    let names = later["changed"].as_array().ok_or("changed")?;
    assert!(!names.contains(&json!("a")), "a reap wakes once: {later:?}");
    Ok(())
}

/// Dies with a reap erasing the move it ends: `a` exits and is reaped between two bare waits
/// while `b` runs, so the second read nothing, slept, and held the plan's notice on `a`.
#[tokio::test]
async fn a_bare_wait_wakes_on_a_child_that_ended_and_was_reaped_unseen() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let root = scratch::Scratch::new("yi-wait-unseen")?;
    let store = support::memory_store("yi-wait-unseen");
    let family = support::family(
        root.to_path_buf(),
        std::env::temp_dir(),
        store,
        Some("sleep 3"),
    );
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let wait = || {
        let payload = kwargs(&[("timeout_ms", json!(1_000))]);
        registry.dispatch("rlm.wait", payload)
    };
    for name in ["a", "b"] {
        let named = kwargs(&[("name", json!(name))]);
        family.host.spawn("work".to_owned(), named)?;
    }
    wait().ok_or("rlm.wait")?.await?;
    family.host.interrupt("a")?;
    assert!(family.reaches("a", "failed").await);
    family.host.delete("a")?;
    let started = Instant::now();
    let again = wait().ok_or("rlm.wait")?.await?;
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "a's end is news to this waiter: {again:?}"
    );
    let names = again["changed"].as_array().ok_or("changed")?;
    assert!(names.contains(&json!("a")), "{again:?}");
    Ok(())
}

/// Dies with `yi ask` holding on `busy`, which a stuck child keeps true: a finished run stayed
/// open until its deadline, 1303 s after the last answer. A moving child of either kind holds.
#[tokio::test]
async fn an_ended_owner_turn_waits_on_a_moving_child() -> TestResult {
    let root = scratch::Scratch::new("yi-ask-hold")?;
    let store = support::memory_store("yi-ask-hold");
    let family = support::family(
        root.to_path_buf(),
        std::env::temp_dir(),
        store,
        Some("sleep 3"),
    );
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("mine"))]))?;
    assert!(family.host.busy());
    assert!(
        family.host.holds_owner(),
        "the model's own rlm.run child holds the run"
    );
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("plan/todo"))]))?;
    assert!(
        family.host.holds_owner(),
        "a moving plan child holds the run"
    );
    Ok(())
}

/// Dies with `settling` bumped inside the finish hook: between the exit and the hook `busy`
/// reads false, and `yi ask` ends before the finish reaches the owner.
#[tokio::test]
async fn a_finishing_child_keeps_the_host_busy_from_its_exit() -> TestResult {
    let (_root, family) = family("yi-settling-gap")?;
    let seen: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = std::sync::Arc::default();
    let (host, told) = (std::sync::Arc::downgrade(&family.host), seen.clone());
    let hook: std::sync::Arc<yi_runtime::subagent::FinishFn> =
        std::sync::Arc::new(move |_name, _exit, _error| {
            if let (Some(host), Ok(mut told)) = (host.upgrade(), told.lock()) {
                told.push(host.busy());
            }
            false
        });
    family.host.set_finished(hook);
    family
        .host
        .spawn("work".to_owned(), kwargs(&[("name", json!("a"))]))?;
    assert!(family.reaches("a", "finished").await);
    assert_eq!(*seen.lock().map_err(|_| "poisoned")?, [true]);
    Ok(())
}

/// Dies with a refused timeout replaced by the five-minute cap: `rlm.wait(-5)` answered
/// `timeout_ms: 300000, clamped: false`, and `rlm.receive(-1)` slept the whole cap unasked.
#[tokio::test]
async fn a_negative_or_non_numeric_timeout_is_refused_naming_it() -> TestResult {
    use yi_kernel::client::HostHandlers;
    let (_root, family) = family("yi-wait-negative")?;
    let mut registry = yi_runtime::HostRegistry::default();
    family.host.register(&mut registry);
    let config = yi_runtime::SessionConfig {
        system_prompt: "sys".to_owned(),
        model: support::faux_model(),
        thinking_level: None,
        tool_execution: yi_loop::ExecutionMode::Sequential,
    };
    let provider = std::sync::Arc::new(yi_runtime::ProviderStream::new(None, None));
    let session = yi_runtime::AgentSession::new(config, provider);
    yi_runtime::mailbox::register_receive(&session, &family.host, &mut registry);
    for asked in [json!(-5_000), json!("soon")] {
        let payload = kwargs(&[
            ("timeout_ms", asked.clone()),
            ("target", json!("a")),
            ("message", json!("hi")),
        ]);
        for verb in ["rlm.wait", "rlm.receive", "agent_message.request"] {
            let call = registry.dispatch(verb, payload.clone()).ok_or(verb)?;
            match tokio::time::timeout(Duration::from_secs(5), call).await {
                Ok(Err(refusal)) => {
                    assert!(refusal.contains(&asked.to_string()), "{verb}: {refusal}")
                }
                Ok(Ok(reply)) => return Err(format!("{verb}({asked}) answered {reply:?}").into()),
                Err(_) => return Err(format!("{verb}({asked}) waited past 5 s").into()),
            }
        }
    }
    Ok(())
}

/// Dies with a miss naming nothing: `rlm.result("no-such")` said only that nothing matched,
/// where `rlm.status` names the children there are.
#[tokio::test]
async fn an_unknown_child_is_refused_naming_the_children_there_are() -> TestResult {
    let (_root, family) = family("yi-no-such-child")?;
    for name in ["b", "a"] {
        let named = kwargs(&[("name", json!(name))]);
        family.host.spawn("work".to_owned(), named)?;
    }
    let refusal = family
        .host
        .result("no-such", None)
        .err()
        .ok_or("no-such matched")?;
    assert_eq!(
        refusal,
        "No RLM child matches \"no-such\"; the children are: a, b"
    );
    Ok(())
}
