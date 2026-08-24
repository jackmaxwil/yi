use std::error::Error;

use serde_json::json;
use yi_context::{
    Bytes, CHILD_USAGE_CAUSE, HarnessState, Prefill, Scope, Settings, Tokens, Window,
    attribute_child_usage, context_tokens, drop_internal, estimate_context, fit, internal_source,
    own_and_total_usage, prepare_compaction, project, retain_floor, select_cut,
    serialize_conversation, should_compact, wrap_internal,
};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, Cost, StopReason, Usage, UserContent};
use yi_types::record::LaneRecord;

type TestResult = Result<(), Box<dyn Error>>;

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text.to_owned()),
        timestamp: 1,
    }
}

fn usage(input: i64, output: i64, total: i64) -> Usage {
    let zero = || serde_json::Number::from(0u64);
    Usage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: total,
        cost: Cost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            total: zero(),
        },
    }
}

fn assistant(text: &str, usage_row: Usage, stop_reason: StopReason) -> AgentMessage {
    AgentMessage::Assistant {
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: usage_row,
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 2,
    }
}

fn tool_result(text: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "call-1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 3,
    }
}

fn message_entry(id: &str, seq: u64, message: AgentMessage) -> Entry {
    Entry::Message {
        id: id.to_owned(),
        message,
        terminate: None,
        parent_id: None,
        seq,
        timestamp: seq,
    }
}

#[test]
fn estimate_uses_last_authoritative_usage_plus_trailing_chars() -> TestResult {
    let messages = vec![
        user("earlier"),
        assistant("reply", usage(100, 50, 1000), StopReason::Stop),
        user("trailing message of forty characters aaaa"),
    ];
    let estimate = estimate_context(&messages);
    assert_eq!(estimate.usage_tokens, Tokens(1000));
    assert_eq!(estimate.last_usage_index, Some(1));
    assert!(estimate.trailing_tokens.0 > 0);
    assert_eq!(
        estimate.tokens,
        estimate
            .usage_tokens
            .saturating_add(estimate.trailing_tokens)
    );
    Ok(())
}

#[test]
fn aborted_and_error_assistants_carry_no_authoritative_usage() -> TestResult {
    let messages = vec![
        assistant("ok", usage(10, 5, 500), StopReason::Stop),
        assistant("boom", usage(999, 999, 9999), StopReason::Error),
        assistant("stopme", usage(999, 999, 9999), StopReason::Aborted),
    ];
    assert_eq!(estimate_context(&messages).usage_tokens, Tokens(500));
    Ok(())
}

#[test]
fn context_tokens_falls_back_to_component_sum() -> TestResult {
    assert_eq!(context_tokens(&usage(100, 50, 0)), Tokens(150));
    assert_eq!(context_tokens(&usage(100, 50, 4000)), Tokens(4000));
    Ok(())
}

#[test]
fn body_after_prefix_scope_subtracts_the_prefill_baseline() -> TestResult {
    let total = Tokens(50_000);
    assert_eq!(
        yi_context::account::scoped_tokens(total, Scope::BodyAfterPrefix, Some(Tokens(30_000))),
        Tokens(20_000)
    );
    assert_eq!(
        yi_context::account::scoped_tokens(total, Scope::Total, Some(Tokens(30_000))),
        total
    );
    Ok(())
}

#[test]
fn projection_applies_the_latest_compaction_and_its_retained_tail() -> TestResult {
    let branch = vec![
        message_entry("m1", 1, user("old request")),
        Entry::Compaction {
            id: "c1".to_owned(),
            summary: "the summary".to_owned(),
            retained_tail: vec![user("kept from before")],
            tokens_before: 12_345,
            details: None,
            usage: None,
            parent_id: Some("m1".to_owned()),
            seq: 2,
            timestamp: 2,
        },
        message_entry("m2", 3, user("after compaction")),
    ];
    let projected = project(&branch);
    assert_eq!(projected.len(), 3);
    assert!(matches!(
        &projected[0],
        AgentMessage::CompactionSummary { summary, tokens_before, .. }
            if summary == "the summary" && *tokens_before == 12_345
    ));
    assert!(
        matches!(&projected[1], AgentMessage::User { content: UserContent::Text(text), .. } if text == "kept from before")
    );
    Ok(())
}

#[test]
fn projection_drops_deferred_assistants_and_non_message_entries() -> TestResult {
    let branch = vec![
        message_entry("m1", 1, user("hi")),
        Entry::ModelChange {
            id: "mc".to_owned(),
            provider: "faux".to_owned(),
            model_id: "faux-1".to_owned(),
            parent_id: None,
            seq: 2,
            timestamp: 2,
        },
        message_entry(
            "m2",
            3,
            assistant("deferred", usage(0, 0, 0), StopReason::Deferred),
        ),
    ];
    assert_eq!(project(&branch).len(), 1);
    Ok(())
}

#[test]
fn should_compact_triggers_inside_the_reserve() -> TestResult {
    let settings = Settings::default();
    assert!(!should_compact(Tokens(100_000), Tokens(128_000), &settings));
    assert!(should_compact(Tokens(120_000), Tokens(128_000), &settings));
    assert!(!should_compact(
        Tokens(120_000),
        Tokens(0),
        &Settings::default()
    ));
    let disabled = Settings {
        enabled: false,
        ..Settings::default()
    };
    assert!(!should_compact(Tokens(120_000), Tokens(128_000), &disabled));
    Ok(())
}

#[test]
fn cut_never_lands_on_a_tool_result() -> TestResult {
    let big = "x".repeat(40_000);
    let messages = vec![
        user("start"),
        assistant(&big, usage(0, 0, 0), StopReason::ToolUse),
        tool_result(&big),
        tool_result("small"),
        assistant("done", usage(0, 0, 0), StopReason::Stop),
    ];
    let cut = select_cut(&messages, Tokens(10_000));
    assert!(!matches!(
        messages[cut.first_kept_index],
        AgentMessage::ToolResult { .. }
    ));
    Ok(())
}

#[test]
fn cut_inside_a_turn_records_the_turn_start_for_the_prefix_summary() -> TestResult {
    let big = "y".repeat(100_000);
    let messages = vec![
        user("the request"),
        assistant(&big, usage(0, 0, 0), StopReason::ToolUse),
        tool_result(&big),
        assistant("tail work", usage(0, 0, 0), StopReason::Stop),
    ];
    let cut = select_cut(&messages, Tokens(5_000));
    assert!(cut.is_split_turn);
    assert_eq!(cut.turn_start_index, Some(0));
    assert!(cut.first_kept_index > 0);
    Ok(())
}

#[test]
fn small_histories_cut_at_the_first_message_and_summarize_nothing() -> TestResult {
    let messages = vec![
        user("hi"),
        assistant("yo", usage(0, 0, 0), StopReason::Stop),
    ];
    let cut = select_cut(&messages, Tokens(20_000));
    assert_eq!(cut.first_kept_index, 0);
    assert!(!cut.is_split_turn);
    Ok(())
}

#[test]
fn serialization_labels_roles_and_truncates_tool_results() -> TestResult {
    let long = "z".repeat(3000);
    let text = serialize_conversation(&[
        user("ask"),
        assistant("answer", usage(0, 0, 0), StopReason::Stop),
        tool_result(&long),
    ]);
    assert!(text.contains("[User]: ask"));
    assert!(text.contains("[Assistant]: answer"));
    assert!(text.contains("[Tool result]:"));
    assert!(text.contains("more characters truncated]"));
    assert!(!text.contains(&long));
    Ok(())
}

#[test]
fn retention_floor_keeps_user_messages_newest_first_and_truncates_the_oldest() -> TestResult {
    let old_big = "a".repeat(400_000);
    let survivors = retain_floor(
        &[
            user(&old_big),
            assistant("noise", usage(0, 0, 0), StopReason::Stop),
            user("recent ask"),
        ],
        Tokens(64_000),
    );
    assert_eq!(survivors.len(), 2);
    assert!(matches!(
        &survivors[1],
        AgentMessage::User { content: UserContent::Text(text), .. } if text == "recent ask"
    ));
    let AgentMessage::User {
        content: UserContent::Text(truncated),
        ..
    } = &survivors[0]
    else {
        return Err("expected user message".into());
    };
    assert!(truncated.contains("characters truncated"));
    assert!(truncated.len() < old_big.len());
    Ok(())
}

#[test]
fn prepare_returns_none_when_there_is_nothing_to_summarize() -> TestResult {
    let branch = vec![message_entry("m1", 1, user("hi"))];
    assert!(prepare_compaction(&branch, &Settings::default()).is_none());
    let compaction_leaf = vec![
        message_entry("m1", 1, user("hi")),
        Entry::Compaction {
            id: "c1".to_owned(),
            summary: "s".to_owned(),
            retained_tail: Vec::new(),
            tokens_before: 0,
            details: None,
            usage: None,
            parent_id: None,
            seq: 2,
            timestamp: 2,
        },
    ];
    assert!(prepare_compaction(&compaction_leaf, &Settings::default()).is_none());
    Ok(())
}

#[test]
fn prepare_splits_history_and_unions_the_floor_into_the_retained_tail() -> TestResult {
    let big = "b".repeat(120_000);
    let branch = vec![
        message_entry("m1", 1, user("first requirement: keep the tests green")),
        message_entry("m2", 2, assistant(&big, usage(0, 0, 0), StopReason::Stop)),
        message_entry("m3", 3, user("second ask")),
        message_entry(
            "m4",
            4,
            assistant("done", usage(100, 50, 40_000), StopReason::Stop),
        ),
    ];
    let settings = Settings {
        keep_recent_tokens: Tokens(100),
        ..Settings::default()
    };
    let prepared = prepare_compaction(&branch, &settings).ok_or("expected preparation")?;
    assert!(
        !prepared.messages_to_summarize.is_empty() || !prepared.turn_prefix_messages.is_empty()
    );
    assert!(
        prepared.retained_tail.iter().any(|message| matches!(
            message,
            AgentMessage::User { content: UserContent::Text(text), .. }
                if text.contains("first requirement")
        )),
        "floored user message must survive into the retained tail"
    );
    assert!(prepared.tokens_before.0 > 0);
    Ok(())
}

#[test]
fn internal_context_wrapper_round_trips_and_dies_at_compaction() -> TestResult {
    let wrapped = wrap_internal("heartbeat", "tick", 1);
    assert_eq!(internal_source(&wrapped), Some("heartbeat"));
    assert_eq!(internal_source(&user("plain")), None);
    let invalid = wrap_internal("Bad Label!", "text", 1);
    assert_eq!(internal_source(&invalid), Some("internal"));
    let kept = drop_internal(&[wrapped, user("plain")]);
    assert_eq!(kept.len(), 1);
    Ok(())
}

#[test]
fn attribution_preserves_the_parent_context_size() -> TestResult {
    let mut parent = usage(100, 50, 150);
    attribute_child_usage(&mut parent, &usage(1000, 500, 1500));
    assert_eq!(parent.input, 1100);
    assert_eq!(parent.output, 550);
    assert_eq!(parent.total_tokens, 150);
    Ok(())
}

#[test]
fn own_and_total_usage_separate_child_attributions() -> TestResult {
    let record = |cause: &str, row: Usage| LaneRecord::Usage {
        id: "r".to_owned(),
        lane: "main".to_owned(),
        usage: row,
        cause: cause.to_owned(),
        run_id: None,
        entry_id: None,
        attempt: None,
        stop_reason: None,
        tool_call_id: None,
        details: None,
        seq: 1,
        timestamp: 1,
    };
    let records = vec![
        record("assistant", usage(100, 50, 150)),
        record(CHILD_USAGE_CAUSE, usage(1000, 500, 1500)),
    ];
    let (own, total) = own_and_total_usage(&records);
    assert_eq!(own.input, 100);
    assert_eq!(total.input, 1100);
    Ok(())
}

#[test]
fn window_chain_advances_ids_and_resets_latches() -> TestResult {
    let mut window = Window::new_initial("w0".to_owned());
    assert!(window.claim_advisory());
    assert!(!window.claim_advisory());
    window.observe_prefill(Prefill::Estimated(Tokens(10)));
    let ids = window.advance("w1".to_owned());
    assert_eq!(ids.first, "w0");
    assert_eq!(ids.previous.as_deref(), Some("w0"));
    assert_eq!(ids.id, "w1");
    assert_eq!(ids.number, 1);
    assert_eq!(window.prefill_tokens(), None);
    assert!(window.claim_advisory());
    window.observe_prefill(Prefill::ServerObserved(Tokens(42)));
    window.observe_prefill(Prefill::Estimated(Tokens(7)));
    assert_eq!(window.prefill_tokens(), Some(Tokens(42)));
    Ok(())
}

#[test]
fn source_budget_fit_marks_truncation() -> TestResult {
    let fitted = fit("abcdef", Bytes(3));
    assert!(fitted.truncated);
    assert!(fitted.text.starts_with("abc"));
    assert!(fitted.text.contains("truncated"));
    assert!(!fit("abc", Bytes(3)).truncated);
    Ok(())
}

#[test]
fn ledger_loads_prime_shaped_state_and_formats_hints() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-ledger-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("harness_state.json");
    std::fs::write(
        &path,
        serde_json::to_string(&json!({
            "entries": {
                "memory": {
                    "m1": {"title": "Build cmd", "content": "use just check", "scope": "global"},
                    "bad": {"content": 42}
                },
                "prompt": {},
                "skill": {"s1": {"title": "ignored kind", "content": "x"}}
            }
        }))?,
    )?;
    let state = HarnessState::load(&path);
    let memory = state
        .entries
        .get(&yi_types::harness::HarnessKind::Memory)
        .ok_or("memory kind missing")?;
    assert_eq!(memory.len(), 1);
    let prompt_text = state
        .format_for_prompt(Bytes(4096))
        .ok_or("expected prompt text")?;
    assert!(prompt_text.contains("[global:m1] Build cmd: use just check"));
    let empty = HarnessState::load(&dir.join("missing.json"));
    assert!(empty.format_for_prompt(Bytes(4096)).is_none());
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
