use std::error::Error;

use serde_json::json;
use yi_context::{
    Attributed, BriefLine, Bytes, CHILD_USAGE_CAUSE, CompiledView, FileOps, HarnessState, Prefill,
    Scope, Settings, Tokens, Window, attribute_child_usage, compile_view, compose_summary,
    context_tokens, drop_internal, estimate_context, fit, internal_source, own_and_total_usage,
    prepare_compaction, project, retain_floor, select_cut, serialize_conversation, should_compact,
    wrap_internal,
};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, Cost, StopReason, Usage, UserContent};
use yi_types::record::LaneRecord;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn user(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 1)
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
        unknown: false,
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

fn assistant_call(name: &str, path: &str) -> AgentMessage {
    let mut arguments = serde_json::Map::new();
    arguments.insert("path".to_owned(), json!(path));
    AgentMessage::Assistant {
        content: vec![Content::ToolCall {
            id: "call-1".to_owned(),
            name: name.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        }],
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: usage(0, 0, 0),
        stop_reason: StopReason::ToolUse,
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
    let reminder = wrap_internal("reminder", "Relevant: skill://plan", 1);
    assert_eq!(internal_source(&reminder), Some("reminder"));
    let AgentMessage::User {
        content: UserContent::Text(text),
        ..
    } = &reminder
    else {
        return Err("a wrapped reminder is a user-role message".into());
    };
    let advisory = text
        .find("not a user instruction")
        .ok_or("no advisory line")?;
    let pointer = text.find("Relevant:").ok_or("no pointer")?;
    assert!(advisory < pointer, "the advisory line comes first: {text}");
    assert_eq!(
        internal_source(&wrap_internal("heartbeat", "tick", 1)),
        Some("heartbeat")
    );
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
fn ledger_loads_reference_shaped_state_and_formats_hints() -> TestResult {
    let dir = Scratch::new("yi-ledger")?;
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
    Ok(())
}

fn att(id: &str, message: AgentMessage) -> Attributed {
    Attributed {
        id: Some(id.to_owned()),
        message,
    }
}

fn brief_texts(view: &CompiledView) -> Vec<String> {
    view.brief.iter().map(BriefLine::render).collect()
}

#[test]
fn compile_view_puts_entry_ids_on_tool_pointers_not_user_prose() -> TestResult {
    let long = "body ".repeat(80);
    let attributed = vec![
        att("m1", user("port the parser")),
        att(
            "m2",
            assistant("working because we must", usage(0, 0, 0), StopReason::Stop),
        ),
        att("t1", tool_result(&long)),
    ];
    let view = compile_view(&attributed, None);
    let rendered = view.render();
    assert!(
        !rendered.contains("port the parser"),
        "P17 already keeps user text; the brief must not re-summarize it: {rendered}"
    );
    assert!(
        !rendered.contains("working because"),
        "assistant prose is not a keyword brief: {rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "(#t1) tool: read → {} chars",
            long.chars().count()
        )),
        "a compacted-away tool result stays addressable by id: {rendered}"
    );
    Ok(())
}

#[test]
fn compile_view_rolls_previous_brief_and_caps_at_120() -> TestResult {
    let previous = CompiledView {
        brief: (0..100)
            .map(|index| BriefLine {
                id: Some(format!("old{index}")),
                text: "tool: probe → 10 chars".to_owned(),
            })
            .collect(),
        ..CompiledView::default()
    };
    let attributed: Vec<Attributed> = (0..30)
        .map(|index| att(&format!("n{index}"), tool_result(&format!("ok {index}"))))
        .collect();
    let view = compile_view(&attributed, Some(&previous));
    assert_eq!(view.brief.len(), 120);
    let lines = brief_texts(&view);
    assert!(
        !lines.iter().any(|line| line.contains("(#old0)")),
        "the oldest previous line must roll off when 100+30 exceeds the cap"
    );
    assert!(
        lines.iter().any(|line| line.contains("(#old10)")),
        "an older id that still fits the rolling window must survive"
    );
    assert!(
        lines.iter().any(|line| line.contains("(#n29)")),
        "the newest summarized tool turn must survive the cap"
    );
    Ok(())
}

#[test]
fn compose_summary_prefixes_view_before_llm_prose() -> TestResult {
    let view = CompiledView {
        brief: vec![BriefLine {
            id: Some("m1".to_owned()),
            text: "tool: read → 4 chars".to_owned(),
        }],
        ..CompiledView::default()
    };
    let (text, _) = compose_summary("## Goal\nPort the parser", &FileOps::default(), &view);
    let view_at = text
        .find("<yi_compact_view>")
        .ok_or("view marker missing")?;
    let goal_at = text.find("## Goal").ok_or("llm prose missing")?;
    assert!(
        view_at < goal_at,
        "the host view must lead the LLM checkpoint so it is not rewritten as Goal prose: {text}"
    );
    Ok(())
}

#[test]
fn compose_summary_persists_the_view_for_the_next_round_to_roll() -> TestResult {
    let view = CompiledView {
        brief: vec![BriefLine {
            id: Some("m1".to_owned()),
            text: "tool: read → 4 chars".to_owned(),
        }],
        earlier: vec!["(#a..#b)".to_owned()],
        ..CompiledView::default()
    };
    let (_, details) = compose_summary("## Goal\nPort", &FileOps::default(), &view);
    let restored = yi_context::view_from_extra(&details.extra).ok_or("view not persisted")?;
    assert_eq!(restored.brief, view.brief);
    assert_eq!(restored.earlier, view.earlier);
    Ok(())
}

#[test]
fn a_successful_result_that_talks_about_errors_is_still_a_pointer() -> TestResult {
    let body = "ran the suite: 0 failed, no error surfaced, every assertion held";
    let view = compile_view(&[att("t1", tool_result(body))], None);
    let rendered = view.render();
    assert!(
        rendered.contains(&format!(
            "(#t1) tool: read → {} chars",
            body.chars().count()
        )),
        "is_error, not the words in the body, decides an outstanding line: {rendered}"
    );
    assert!(
        !rendered.contains("0 failed"),
        "a successful body must not ride the brief: {rendered}"
    );
    assert!(
        view.outstanding.is_empty(),
        "nothing is outstanding: {rendered}"
    );
    Ok(())
}

#[test]
fn tool_result_is_a_pointer_and_a_later_edit_marks_the_read_stale() -> TestResult {
    let long = "body ".repeat(80);
    let attributed = vec![
        att("u1", user("must keep the latch")),
        att("r1", assistant_call("read", "src/latch.rs")),
        att("t1", tool_result(&long)),
        att("e1", assistant_call("edit", "src/latch.rs")),
        att(
            "a1",
            assistant(
                "First sentence is filler. We decided the latch because the kernel must never restart.",
                usage(0, 0, 0),
                StopReason::Stop,
            ),
        ),
    ];
    let view = compile_view(&attributed, None);
    let rendered = view.render();
    assert!(
        rendered.contains(&format!(
            "(#t1) tool: read → {} chars",
            long.chars().count()
        )),
        "a successful tool result is a pointer, not the body: {rendered}"
    );
    assert!(
        !rendered.contains("body body body"),
        "the result body must not ride the brief: {rendered}"
    );
    assert!(
        rendered.contains("(#r1) read src/latch.rs stale"),
        "a later edit marks the read stale: {rendered}"
    );
    assert!(
        rendered.contains("(#e1) edit src/latch.rs"),
        "an edit is a path pointer: {rendered}"
    );
    assert!(
        !rendered.contains("decided the latch"),
        "assistant decisions are not keyword-kept: {rendered}"
    );
    assert!(
        rendered.contains("[Kernel]") && rendered.contains("IPython kernel keeps running"),
        "the view carries the kernel persist note: {rendered}"
    );
    Ok(())
}

#[test]
fn earlier_index_names_the_demoted_span() -> TestResult {
    let attributed: Vec<Attributed> = (0..125)
        .map(|index| att(&format!("t{index}"), tool_result(&format!("ok {index}"))))
        .collect();
    let view = compile_view(&attributed, None);
    assert_eq!(view.brief.len(), 120);
    assert_eq!(view.earlier.len(), 1);
    assert_eq!(
        view.earlier[0], "(#t0..#t4)",
        "demoted lines collapse to one index line with exact ids"
    );
    Ok(())
}

#[test]
fn a_user_only_span_still_renders_a_kernel_line() -> TestResult {
    let view = compile_view(&[att("only", user("hi"))], None);
    let rendered = view.render();
    assert!(!rendered.contains("(#only)"), "{rendered}");
    assert!(rendered.contains("[Kernel]"), "{rendered}");
    assert!(view.earlier.is_empty());
    assert!(view.brief.is_empty());
    Ok(())
}

#[test]
fn a_retained_tail_message_is_briefed_without_a_pointer_to_the_compaction() -> TestResult {
    let branch = vec![
        Entry::Compaction {
            id: "c1".to_owned(),
            summary: "earlier work".to_owned(),
            retained_tail: vec![tool_result("tail body")],
            tokens_before: 10,
            usage: None,
            details: None,
            parent_id: None,
            seq: 0,
            timestamp: 1,
        },
        message_entry("m9", 2, tool_result("later body")),
    ];
    let view = compile_view(&yi_context::project_attributed(&branch), None);
    let lines = brief_texts(&view);
    assert!(
        lines.iter().any(|line| line == "tool: read → 9 chars"),
        "the tail message is briefed with no id at all: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("(#c1)")),
        "a tail message predates the compaction and must not cite it: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("(#m9)")),
        "a real entry still carries its pointer: {lines:?}"
    );
    Ok(())
}
