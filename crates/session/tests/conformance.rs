use std::error::Error;

use serde_json::{Value, json};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::record::{LaneRecord, OperationIntent};
use yi_types::wire::Mutation;

use yi_session::{
    BranchBounds, CreateOptions, EntryOrder, EntryQuery, ForkPosition, ForkScope, JsonlRepo,
    LanePointer, LogOptions, MemRepo, RecordQuery, SessionError, SessionRepo, lock_session,
};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn for_each_backend(case: impl Fn(&mut dyn SessionRepo) -> TestResult) -> TestResult {
    let mut mem = MemRepo::new();
    case(&mut mem)?;
    let root = Scratch::new("yi-session-conformance")?;
    let mut jsonl = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-conformance");
    case(&mut jsonl)?;
    Ok(())
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::user_input(
        UserContent::Blocks(vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }]),
        1,
    )
}

fn assistant_message(text: &str, usage: Usage) -> AgentMessage {
    AgentMessage::Assistant {
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "claude-sonnet-4-5".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    }
}

fn message_entry(id: &str, text: &str) -> Entry {
    Entry::Message {
        id: id.to_owned(),
        message: user_message(text),
        terminate: None,
        parent_id: None,
        seq: 0,
        timestamp: 0,
    }
}

fn custom_entry(id: &str, custom_type: &str, data: Option<Value>) -> Entry {
    Entry::Custom {
        id: id.to_owned(),
        custom_type: custom_type.to_owned(),
        data,
        parent_id: None,
        seq: 0,
        timestamp: 0,
    }
}

fn run_started(id: &str, lane: &str) -> LaneRecord {
    LaneRecord::OperationStarted {
        id: id.to_owned(),
        lane: lane.to_owned(),
        source_leaf_id: None,
        intent: OperationIntent::Run {
            original_prompt: Vec::new(),
            initial_messages: Vec::new(),
            system_prompt_override: None,
            resume_data: None,
        },
        seq: 0,
        timestamp: 0,
    }
}

fn op_started(id: &str, lane: &str, kind: &str) -> LaneRecord {
    let intent = match kind {
        "compaction" => OperationIntent::Compaction {
            custom_instructions: None,
            result_entry_id: format!("{id}-result"),
        },
        "navigation" => OperationIntent::Navigation {
            target_id: None,
            summarize: false,
            custom_instructions: None,
            label: None,
            summary_entry_id: None,
        },
        _ => {
            return run_started(id, lane);
        }
    };
    LaneRecord::OperationStarted {
        id: id.to_owned(),
        lane: lane.to_owned(),
        source_leaf_id: None,
        intent,
        seq: 0,
        timestamp: 0,
    }
}

fn op_finished(id: &str, lane: &str, run_id: &str) -> LaneRecord {
    LaneRecord::OperationFinished {
        id: id.to_owned(),
        lane: lane.to_owned(),
        run_id: run_id.to_owned(),
        outcome: "completed".to_owned(),
        error: None,
        seq: 0,
        timestamp: 0,
    }
}

fn usage_with(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total: i64,
    cost_total: f64,
) -> Usage {
    let mut usage = Usage::zero();
    usage.input = input;
    usage.output = output;
    usage.cache_read = cache_read;
    usage.cache_write = cache_write;
    usage.total_tokens = total;
    usage.cost.total =
        serde_json::Number::from_f64(cost_total).unwrap_or_else(|| serde_json::Number::from(0u64));
    usage
}

fn usage_record(id: &str, lane: &str, cause: &str, usage: Usage) -> LaneRecord {
    LaneRecord::Usage {
        id: id.to_owned(),
        lane: lane.to_owned(),
        usage,
        cause: cause.to_owned(),
        run_id: None,
        entry_id: None,
        attempt: None,
        stop_reason: None,
        tool_call_id: None,
        details: None,
        seq: 0,
        timestamp: 0,
    }
}

fn entry_ids(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(Entry::id).collect()
}

fn record_ids(records: &[LaneRecord]) -> Vec<&str> {
    records.iter().map(LaneRecord::id).collect()
}

fn assert_code<T>(result: Result<T, SessionError>, code: &str) -> TestResult {
    match result {
        Ok(_) => Err(format!("expected {code} error, got Ok").into()),
        Err(error) if error.code() == code => Ok(()),
        Err(error) => Err(format!("expected {code} error, got {}: {error}", error.code()).into()),
    }
}

fn lane_pointer(lane: &str, leaf: Option<&str>) -> LanePointer {
    LanePointer {
        lane: lane.to_owned(),
        leaf_id: leaf.map(str::to_owned),
    }
}

fn mutation_seq(mutation: &Mutation) -> u64 {
    match mutation {
        Mutation::Entry { entry, .. } => entry.seq(),
        Mutation::Record { record } => record.seq(),
        Mutation::Lane { seq, .. } | Mutation::Fact { seq, .. } => *seq,
    }
}

fn mutation_kind(mutation: &Mutation) -> &'static str {
    match mutation {
        Mutation::Entry { .. } => "entry",
        Mutation::Record { .. } => "record",
        Mutation::Lane { .. } => "lane",
        Mutation::Fact { .. } => "fact",
    }
}

fn create(repo: &mut dyn SessionRepo, id: &str) -> Result<yi_session::SharedSession, SessionError> {
    repo.create(CreateOptions {
        id: Some(id.to_owned()),
        ..CreateOptions::default()
    })
}

#[test]
fn grep_finds_needles_across_entry_kinds_oldest_first() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let user = session.append_entry(message_entry("u1", "alpha needle here"), "main")?;
        session.append_entry(message_entry("u2", "nothing relevant"), "main")?;
        let summary = session.append_compaction(
            "main",
            "a summary mentioning the needle".to_owned(),
            Vec::new(),
            10,
            None,
        )?;

        let hits = session.grep("needle", 8);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec![user.id(), summary.as_str()]
        );
        assert_eq!(hits[0].entry_type, "message");
        assert_eq!(hits[1].entry_type, "compaction");
        assert_eq!(hits[0].snippet, "alpha needle here");

        assert!(session.grep("   ", 8).is_empty(), "blank needle, no hits");
        assert!(session.grep("absent", 8).is_empty());
        assert_eq!(
            session.grep("needle", 1).len(),
            1,
            "limit clamps the hit count"
        );
        Ok(())
    })
}

#[test]
fn grep_skips_thinking_and_centers_long_snippets_on_the_needle() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let mut thinking_only = assistant_message("visible text", Usage::zero());
        if let AgentMessage::Assistant { content, .. } = &mut thinking_only {
            content.push(Content::Thinking {
                thinking: "needle hidden in thinking".to_owned(),
                thinking_signature: None,
                redacted: None,
            });
        }
        session.append_entry(
            Entry::Message {
                id: "t1".to_owned(),
                message: thinking_only,
                terminate: None,
                parent_id: None,
                seq: 0,
                timestamp: 0,
            },
            "main",
        )?;
        assert!(
            session.grep("needle", 8).is_empty(),
            "thinking blocks stay out of grep"
        );

        let long_line = format!("{} needle {}", "a".repeat(200), "b".repeat(200));
        session.append_entry(message_entry("u9", &long_line), "main")?;
        let hits = session.grep("needle", 8);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet.chars().count(), 160);
        assert!(
            hits[0].snippet.contains("needle"),
            "a long line keeps the needle in the window: {}",
            hits[0].snippet
        );
        Ok(())
    })
}

#[test]
fn assigns_parents_and_one_sequence_across_every_mutation() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let root = session.append_entry(message_entry("root", "root"), "main")?;
        session.create_lane("thread", Some("root"))?;
        let child = session.append_entry(
            custom_entry("child", "note", Some(json!({"value": 1}))),
            "thread",
        )?;
        let record = session.append_record(run_started("run", "thread"))?;
        session.set_name(Some("Example".to_owned()))?;
        session.set_label("root", Some("checkpoint".to_owned()))?;
        session.move_lane("main", Some("child"))?;

        assert_eq!((root.parent_id(), root.seq()), (None, 1));
        assert_eq!((child.parent_id(), child.seq()), (Some("root"), 3));
        assert_eq!(record.seq(), 4);
        let log = session.log(&LogOptions::default())?;
        let shape: Vec<(&str, u64)> = log
            .iter()
            .map(|m| (mutation_kind(m), mutation_seq(m)))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("entry", 1),
                ("lane", 2),
                ("entry", 3),
                ("record", 4),
                ("fact", 5),
                ("fact", 6),
                ("lane", 7),
            ]
        );
        assert_eq!(
            session.lanes(),
            vec![
                lane_pointer("main", Some("child")),
                lane_pointer("thread", Some("child"))
            ]
        );
        Ok(())
    })
}

#[test]
fn commits_records_and_lane_moves_as_separate_mutations() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_entry(message_entry("root", "root"), "main")?;
        let finished = session.append_record(op_finished("finish", "main", "run"))?;

        assert_eq!(finished.seq(), 2);
        assert_eq!(session.lanes(), vec![lane_pointer("main", Some("root"))]);
        session.move_lane("main", None)?;
        assert_eq!(session.lanes(), vec![lane_pointer("main", None)]);
        assert_code(session.move_lane("main", Some("missing")), "not_found")?;
        assert_eq!(session.find_records(&RecordQuery::default())?.len(), 1);
        let seqs: Vec<u64> = session
            .log(&LogOptions::default())?
            .iter()
            .map(mutation_seq)
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        Ok(())
    })
}

#[test]
fn rejects_duplicate_ids_without_changing_state() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_entry(message_entry("shared", "root"), "main")?;
        assert_code(
            session.append_record(run_started("shared", "main")),
            "already_exists",
        )?;
        session.append_record(run_started("run", "main"))?;
        assert_code(
            session.append_entry(custom_entry("run", "note", None), "main"),
            "already_exists",
        )?;
        let seqs: Vec<u64> = session
            .log(&LogOptions::default())?
            .iter()
            .map(mutation_seq)
            .collect();
        assert_eq!(seqs, vec![1, 2]);
        Ok(())
    })
}

#[test]
fn isolates_lanes_while_sharing_the_tree() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_entry(message_entry("root", "root"), "main")?;
        session.create_lane("thread", Some("root"))?;
        session.append_entry(message_entry("main-child", "main"), "main")?;
        session.append_entry(message_entry("thread-child", "thread"), "thread")?;

        assert_eq!(
            session.lanes(),
            vec![
                lane_pointer("main", Some("main-child")),
                lane_pointer("thread", Some("thread-child")),
            ]
        );
        let oldest = EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        };
        let main_branch = session.find_entries_on_branch(
            "main",
            &oldest,
            &BranchBounds {
                start: Some("main-child".to_owned()),
                ..BranchBounds::default()
            },
        )?;
        assert_eq!(entry_ids(&main_branch), vec!["root", "main-child"]);
        let thread_branch = session.find_entries_on_branch(
            "main",
            &oldest,
            &BranchBounds {
                start: Some("thread-child".to_owned()),
                ..BranchBounds::default()
            },
        )?;
        assert_eq!(entry_ids(&thread_branch), vec!["root", "thread-child"]);
        Ok(())
    })
}

#[test]
fn rejects_invalid_queries_before_empty_reads() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "invalid-queries")?;
        let mut session = lock_session(&session);
        session.create_lane("thread", None)?;

        let zero_limit = EntryQuery {
            limit: Some(0),
            ..EntryQuery::default()
        };
        assert_code(session.find_entries(&zero_limit), "invalid_query")?;
        assert_code(
            session.find_entries_on_branch("main", &zero_limit, &BranchBounds::default()),
            "invalid_query",
        )?;
        assert_code(
            session.find_records(&RecordQuery {
                limit: Some(0),
                ..RecordQuery::default()
            }),
            "invalid_query",
        )?;
        assert_code(
            session.find_records(&RecordQuery {
                operation_kind: Some("run"),
                ..RecordQuery::default()
            }),
            "invalid_query",
        )?;
        assert_code(
            session.find_records(&RecordQuery {
                record_type: Some("step_attempt"),
                operation_kind: Some("run"),
                ..RecordQuery::default()
            }),
            "invalid_query",
        )?;
        assert_code(
            session.find_open_operations("main", Some(0)),
            "invalid_query",
        )?;
        assert_code(
            session.log(&LogOptions {
                limit: Some(0),
                after_seq: None,
            }),
            "invalid_query",
        )?;
        Ok(())
    })
}

#[test]
fn supports_bounded_filtered_and_cursor_based_queries() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_entry(message_entry("root", "root"), "main")?;
        session.append_entry(custom_entry("old-note", "note", Some(json!(1))), "main")?;
        session.append_entry(
            Entry::Compaction {
                id: "compact".to_owned(),
                summary: "summary".to_owned(),
                retained_tail: Vec::new(),
                tokens_before: 10,
                details: None,
                usage: None,
                parent_id: None,
                seq: 0,
                timestamp: 0,
            },
            "main",
        )?;
        session.append_entry(custom_entry("new-note", "note", Some(json!(2))), "main")?;
        session.append_entry(
            Entry::Message {
                id: "tail".to_owned(),
                message: assistant_message("tail", Usage::zero()),
                terminate: None,
                parent_id: None,
                seq: 0,
                timestamp: 0,
            },
            "main",
        )?;

        assert_eq!(
            entry_ids(&session.find_entries(&EntryQuery::default())?),
            vec!["tail", "new-note", "compact", "old-note", "root"]
        );
        assert_eq!(
            entry_ids(&session.find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                after_seq: Some(2),
                limit: Some(2),
                ..EntryQuery::default()
            })?),
            vec!["compact", "new-note"]
        );
        assert_eq!(
            entry_ids(&session.find_entries(&EntryQuery {
                custom_type: Some("note".to_owned()),
                ..EntryQuery::default()
            })?),
            vec!["new-note", "old-note"]
        );
        let start = |id: &str| BranchBounds {
            start: Some(id.to_owned()),
            ..BranchBounds::default()
        };
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "main",
                &EntryQuery {
                    custom_type: Some("note".to_owned()),
                    limit: Some(1),
                    ..EntryQuery::default()
                },
                &start("tail"),
            )?),
            vec!["new-note"]
        );
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "main",
                &EntryQuery {
                    entry_type: Some("message"),
                    ..EntryQuery::default()
                },
                &BranchBounds {
                    start: Some("tail".to_owned()),
                    stop_at_type: Some("compaction"),
                    stop_at_id: None,
                },
            )?),
            vec!["tail"]
        );
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "main",
                &EntryQuery {
                    entry_type: Some("custom"),
                    ..EntryQuery::default()
                },
                &BranchBounds {
                    start: Some("tail".to_owned()),
                    stop_at_type: None,
                    stop_at_id: Some("tail".to_owned()),
                },
            )?),
            Vec::<&str>::new()
        );
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "main",
                &EntryQuery {
                    order: EntryOrder::OldestFirst,
                    ..EntryQuery::default()
                },
                &BranchBounds {
                    start: Some("tail".to_owned()),
                    stop_at_type: Some("custom"),
                    stop_at_id: None,
                },
            )?),
            vec!["root", "old-note"]
        );
        assert_code(
            session.find_entries_on_branch("main", &EntryQuery::default(), &start("missing")),
            "not_found",
        )?;
        Ok(())
    })
}

#[test]
fn keeps_lane_names_permanent_with_their_recovery_records() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.create_lane("thread", None)?;
        session.append_record(run_started("old-run", "thread"))?;
        session.append_record(LaneRecord::QueueEnqueued {
            id: "old-next-run".to_owned(),
            lane: "thread".to_owned(),
            queue: "nextRun".to_owned(),
            run_id: None,
            target: json!({"type": "message", "id": "queued-message"}),
            seq: 0,
            timestamp: 0,
        })?;

        let by_lane = session.find_records(&RecordQuery {
            lane: Some("thread".to_owned()),
            ..RecordQuery::default()
        })?;
        assert_eq!(record_ids(&by_lane), vec!["old-next-run", "old-run"]);
        assert_code(session.create_lane("thread", None), "already_exists")?;
        Ok(())
    })
}

#[test]
fn persists_queue_cancellation_without_consuming_its_target() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_record(LaneRecord::QueueEnqueued {
            id: "enqueue".to_owned(),
            lane: "main".to_owned(),
            queue: "nextRun".to_owned(),
            run_id: None,
            target: json!({"type": "message", "id": "queued-message"}),
            seq: 0,
            timestamp: 0,
        })?;
        let cancelled = session.append_record(LaneRecord::QueueCancelled {
            id: "cancel".to_owned(),
            lane: "main".to_owned(),
            run_id: None,
            entry_id: "queued-message".to_owned(),
            seq: 0,
            timestamp: 0,
        })?;
        assert_eq!(cancelled.seq(), 2);
        assert!(session.entry("queued-message").is_none());
        let cancellations = session.find_records(&RecordQuery {
            record_type: Some("queue_cancelled"),
            ..RecordQuery::default()
        })?;
        assert_eq!(cancellations, vec![cancelled]);
        Ok(())
    })
}

#[test]
fn filters_records_by_lane_type_run_sequence_and_order() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_record(run_started("run-1", "main"))?;
        session.append_record(LaneRecord::StepAttempt {
            id: "attempt-1".to_owned(),
            lane: "main".to_owned(),
            run_id: "run-1".to_owned(),
            step: "assistant".to_owned(),
            attempt: 1,
            result_entry_id: "assistant-1".to_owned(),
            compaction_reason: None,
            seq: 0,
            timestamp: 0,
        })?;
        session.create_lane("thread", None)?;
        session.append_record(run_started("run-2", "thread"))?;
        session.append_record(LaneRecord::StepAttempt {
            id: "attempt-2".to_owned(),
            lane: "thread".to_owned(),
            run_id: "run-2".to_owned(),
            step: "assistant".to_owned(),
            attempt: 1,
            result_entry_id: "assistant-2".to_owned(),
            compaction_reason: None,
            seq: 0,
            timestamp: 0,
        })?;

        let thread = session.find_records(&RecordQuery {
            lane: Some("thread".to_owned()),
            ..RecordQuery::default()
        })?;
        assert_eq!(record_ids(&thread), vec!["attempt-2", "run-2"]);
        let attempts = session.find_records(&RecordQuery {
            record_type: Some("step_attempt"),
            order: EntryOrder::OldestFirst,
            ..RecordQuery::default()
        })?;
        assert_eq!(record_ids(&attempts), vec!["attempt-1", "attempt-2"]);
        let after = session.find_records(&RecordQuery {
            run_id: Some("run-1".to_owned()),
            after_seq: Some(1),
            ..RecordQuery::default()
        })?;
        assert_eq!(record_ids(&after), vec!["attempt-1"]);
        let limited = session.find_records(&RecordQuery {
            limit: Some(1),
            ..RecordQuery::default()
        })?;
        assert_eq!(record_ids(&limited), vec!["attempt-2"]);
        Ok(())
    })
}

#[test]
fn filters_operation_starts_by_operation_kind() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_record(op_started("run-old", "main", "run"))?;
        session.append_record(op_finished("run-old-finished", "main", "run-old"))?;
        session.append_record(op_started("compaction", "main", "compaction"))?;
        session.append_record(op_finished("compaction-finished", "main", "compaction"))?;
        session.append_record(op_started("navigation", "main", "navigation"))?;
        session.append_record(op_finished("navigation-finished", "main", "navigation"))?;
        session.append_record(op_started("run-new", "main", "run"))?;

        let starts = |kind: &'static str, order: EntryOrder, limit: Option<usize>| RecordQuery {
            record_type: Some("operation_started"),
            operation_kind: Some(kind),
            order,
            limit,
            ..RecordQuery::default()
        };
        assert_eq!(
            record_ids(&session.find_records(&starts("run", EntryOrder::OldestFirst, None))?),
            vec!["run-old", "run-new"]
        );
        assert_eq!(
            record_ids(&session.find_records(&starts(
                "compaction",
                EntryOrder::NewestFirst,
                None
            ))?),
            vec!["compaction"]
        );
        assert_eq!(
            record_ids(&session.find_records(&starts(
                "navigation",
                EntryOrder::NewestFirst,
                None
            ))?),
            vec!["navigation"]
        );
        assert_eq!(
            record_ids(&session.find_records(&starts("run", EntryOrder::NewestFirst, Some(1)))?),
            vec!["run-new"]
        );
        Ok(())
    })
}

#[test]
fn tracks_and_enforces_one_open_operation_per_lane() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        assert!(session.find_open_operations("main", Some(2))?.is_empty());

        let first = session.append_record(run_started("first", "main"))?;
        assert_eq!(
            session.find_open_operations("main", Some(2))?,
            vec![first.clone()]
        );
        assert_code(
            session.append_record(run_started("second", "main")),
            "storage",
        )?;
        assert_eq!(
            session.find_open_operations("main", Some(2))?,
            vec![first.clone()]
        );

        session.append_record(op_finished("finish-first", "main", first.id()))?;
        assert!(session.find_open_operations("main", Some(2))?.is_empty());
        Ok(())
    })
}

#[test]
fn does_not_let_an_earlier_finish_close_a_later_start() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.append_record(op_finished("finish-before-start", "main", "run"))?;
        let started = session.append_record(run_started("run", "main"))?;
        assert_eq!(
            session.find_open_operations("main", Some(2))?,
            vec![started]
        );
        Ok(())
    })
}

#[test]
fn scopes_open_operations_by_lane_and_limit() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        session.create_lane("thread", None)?;
        let main_run = session.append_record(run_started("main-run", "main"))?;
        let thread_navigation =
            session.append_record(op_started("thread-navigation", "thread", "navigation"))?;

        assert_eq!(
            session.find_open_operations("main", None)?,
            vec![main_run.clone()]
        );
        assert_eq!(
            session.find_open_operations("main", Some(1))?,
            vec![main_run]
        );
        assert_eq!(
            session.find_open_operations("thread", Some(2))?,
            vec![thread_navigation]
        );
        Ok(())
    })
}

#[test]
fn keeps_latest_value_facts_and_computes_ledger_statistics() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let usage = usage_with(10, 5, 3, 2, 20, 10.0);
        session.append_entry(message_entry("user", "question"), "main")?;
        session.append_entry(
            Entry::Message {
                id: "assistant".to_owned(),
                message: assistant_message("answer", usage.clone()),
                terminate: None,
                parent_id: None,
                seq: 0,
                timestamp: 0,
            },
            "main",
        )?;
        session.append_record(usage_record("assistant-usage", "main", "assistant", usage))?;
        session.append_record(usage_record(
            "deferred-usage",
            "main",
            "deferred_fetch",
            Usage::zero(),
        ))?;
        session.create_lane("thread", Some("assistant"))?;
        session.append_record(usage_record(
            "correction",
            "thread",
            "adjustment",
            usage_with(-2, 0, 0, 0, -2, -0.5),
        ))?;
        session.set_name(Some("First".to_owned()))?;
        session.set_name(Some("Second".to_owned()))?;
        session.set_label("user", Some("keep".to_owned()))?;
        session.set_label("user", None)?;
        assert_code(
            session.set_label("missing", Some("checkpoint".to_owned())),
            "not_found",
        )?;

        assert_eq!(session.name().as_deref(), Some("Second"));
        assert_eq!(session.label("user"), None);
        let usage_records = session.find_records(&RecordQuery {
            record_type: Some("usage"),
            order: EntryOrder::OldestFirst,
            ..RecordQuery::default()
        })?;
        assert_eq!(
            record_ids(&usage_records),
            vec!["assistant-usage", "deferred-usage", "correction"]
        );
        let stats = session.stats();
        assert_eq!(stats.message_count, 2);
        assert_eq!(stats.cached_tokens, 3);
        assert_eq!(stats.uncached_tokens, 10);
        assert_eq!(stats.total_tokens, 18);
        assert!((stats.cost_total - 9.5).abs() < 1e-9);
        Ok(())
    })
}

#[test]
fn clears_session_names_durably() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        {
            let mut session = lock_session(&session);
            session.set_name(Some("Temporary".to_owned()))?;
            session.set_name(None)?;
            assert_eq!(session.name(), None);
        }

        let reopened = repo.open("session")?;
        {
            let reopened = lock_session(&reopened);
            assert_eq!(reopened.name(), None);
            let log = reopened.log(&LogOptions::default())?;
            assert_eq!(log.len(), 2);
            assert_eq!(mutation_kind(&log[0]), "fact");
        }

        let fork = repo.fork(
            "session",
            &ForkScope::default(),
            CreateOptions {
                id: Some("fork".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        assert_eq!(lock_session(&fork).name(), None);
        Ok(())
    })
}

#[test]
fn validates_lane_lifecycle_and_targets() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        assert_code(session.create_lane("main", None), "already_exists")?;
        assert_code(session.create_lane("thread", Some("missing")), "not_found")?;
        assert_code(session.move_lane("missing", None), "invalid_lane")?;
        Ok(())
    })
}

#[test]
fn appends_messages_per_lane_without_caching_leaves() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let root = session.append_message("main", user_message("root"))?;
        session.create_lane("thread", Some(&root))?;
        let main_child = session.append_message("main", user_message("main"))?;
        let thread_child = session.append_message("thread", user_message("thread"))?;

        assert_eq!(session.leaf_id("main")?, Some(main_child.clone()));
        assert_eq!(session.leaf_id("thread")?, Some(thread_child.clone()));
        let oldest = EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        };
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "main",
                &oldest,
                &BranchBounds::default()
            )?),
            vec![root.as_str(), main_child.as_str()]
        );
        assert_eq!(
            entry_ids(&session.find_entries_on_branch(
                "thread",
                &oldest,
                &BranchBounds::default()
            )?),
            vec![root.as_str(), thread_child.as_str()]
        );
        drop(session);
        let empty = create(repo, "empty")?;
        let empty = lock_session(&empty);
        assert!(
            empty
                .find_entries_on_branch("main", &EntryQuery::default(), &BranchBounds::default())?
                .is_empty()
        );
        Ok(())
    })
}

#[test]
fn appends_provisioned_entries_with_their_existing_ids() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let entry = session.append_entry(
            custom_entry("provisioned", "note", Some(json!({"value": 1}))),
            "main",
        )?;
        assert_eq!(
            (entry.id(), entry.parent_id(), entry.seq()),
            ("provisioned", None, 1)
        );
        assert_eq!(session.leaf_id("main")?, Some("provisioned".to_owned()));
        Ok(())
    })
}

#[test]
fn persists_tool_result_termination_decisions() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "session")?;
        let mut session = lock_session(&session);
        let entry = session.append_entry(
            Entry::Message {
                id: "tool-result".to_owned(),
                message: AgentMessage::ToolResult {
                    tool_call_id: "call-1".to_owned(),
                    tool_name: "example".to_owned(),
                    content: vec![Content::Text {
                        text: "done".to_owned(),
                        text_signature: None,
                    }],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    is_error: false,
                    timestamp: 1,
                },
                terminate: Some(true),
                parent_id: None,
                seq: 0,
                timestamp: 0,
            },
            "main",
        )?;
        let stored = session.entry(entry.id()).ok_or("missing stored entry")?;
        let Entry::Message { terminate, .. } = &stored else {
            return Err("expected message entry".into());
        };
        assert_eq!(*terminate, Some(true));
        assert_eq!(session.find_entries(&EntryQuery::default())?, vec![entry]);
        Ok(())
    })
}

#[test]
fn creates_lists_and_opens_sessions() -> TestResult {
    for_each_backend(|repo| {
        let session = create(repo, "one")?;
        let (entry_id, metadata) = {
            let mut session = lock_session(&session);
            let entry_id = session.append_message("main", user_message("persisted"))?;
            (entry_id, session.metadata().clone())
        };

        let listed = repo.list()?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, metadata.id);
        assert_eq!(listed[0].created_at, metadata.created_at);
        assert_eq!(listed[0].parent_session_id, metadata.parent_session_id);
        let reopened = repo.open("one")?;
        assert_eq!(
            entry_ids(&lock_session(&reopened).find_entries(&EntryQuery::default())?),
            vec![entry_id.as_str()]
        );
        assert_code(create(repo, "one"), "already_exists")?;
        Ok(())
    })
}

#[test]
fn deletes_sessions_idempotently() -> TestResult {
    for_each_backend(|repo| {
        create(repo, "one")?;
        repo.delete("one")?;
        assert_code(repo.open("one"), "not_found")?;
        repo.delete("one")?;
        Ok(())
    })
}

#[test]
fn forks_one_branch_with_selected_facts_and_no_records() -> TestResult {
    for_each_backend(|repo| {
        let source = create(repo, "source")?;
        let (root, shared, thread_child, main_child) = {
            let mut source = lock_session(&source);
            let root = source.append_message("main", user_message("root"))?;
            let shared =
                source.append_message("main", assistant_message("shared", Usage::zero()))?;
            source.create_lane("thread", Some(&shared))?;
            let thread_child = source.append_message("thread", user_message("thread"))?;
            let main_child = source.append_message("main", user_message("main"))?;
            source.set_name(Some("Source".to_owned()))?;
            source.set_label(&shared, Some("copied".to_owned()))?;
            source.set_label(&thread_child, Some("excluded".to_owned()))?;
            source.append_record(run_started("run", "main"))?;
            source.append_record(usage_record(
                "source-usage",
                "main",
                "adjustment",
                usage_with(10, 5, 3, 2, 20, 10.0),
            ))?;
            (root, shared, thread_child, main_child)
        };

        let fork = repo.fork(
            "source",
            &ForkScope::Branch {
                entry_id: Some(main_child.clone()),
                position: Some(ForkPosition::At),
            },
            CreateOptions {
                id: Some("branch-fork".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        let mut fork = lock_session(&fork);
        assert_eq!(
            entry_ids(&fork.find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })?),
            vec![root.as_str(), shared.as_str(), main_child.as_str()]
        );
        assert_eq!(fork.lanes(), vec![lane_pointer("main", Some(&main_child))]);
        assert_eq!(fork.name().as_deref(), Some("Source"));
        assert_eq!(fork.label(&shared).as_deref(), Some("copied"));
        assert_eq!(fork.label(&thread_child), None);
        assert!(fork.find_records(&RecordQuery::default())?.is_empty());
        let stats = fork.stats();
        assert_eq!(stats.message_count, 3);
        assert_eq!(stats.total_tokens, 0);
        fork.append_message("main", user_message("after fork"))?;
        assert_eq!(fork.stats().message_count, 4);
        let metadata = fork.metadata().clone();
        assert_eq!(metadata.id, "branch-fork");
        assert_eq!(metadata.parent_session_id.as_deref(), Some("source"));
        Ok(())
    })
}

#[test]
fn forks_a_complete_tree_with_lanes_and_facts() -> TestResult {
    for_each_backend(|repo| {
        let source = create(repo, "source")?;
        let (root, main_child, thread_child) = {
            let mut source = lock_session(&source);
            let root = source.append_message("main", user_message("root"))?;
            source.create_lane("thread", Some(&root))?;
            let main_child = source.append_message("main", user_message("main"))?;
            let thread_child = source.append_message("thread", user_message("thread"))?;
            source.set_label(&thread_child, Some("thread-tip".to_owned()))?;
            (root, main_child, thread_child)
        };

        let fork = repo.fork(
            "source",
            &ForkScope::Tree,
            CreateOptions {
                id: Some("tree-fork".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        let fork = lock_session(&fork);
        assert_eq!(
            entry_ids(&fork.find_entries(&EntryQuery {
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })?),
            vec![root.as_str(), main_child.as_str(), thread_child.as_str()]
        );
        assert_eq!(
            fork.lanes(),
            vec![
                lane_pointer("main", Some(&main_child)),
                lane_pointer("thread", Some(&thread_child)),
            ]
        );
        assert_eq!(fork.label(&thread_child).as_deref(), Some("thread-tip"));
        assert_eq!(fork.stats().message_count, 3);
        let lane_mutations: Vec<(u64, &str)> = fork
            .log(&LogOptions::default())?
            .iter()
            .filter_map(|mutation| match mutation {
                Mutation::Lane { seq, lane, .. } => Some((*seq, lane.as_str())),
                _ => None,
            })
            .map(|(seq, lane)| (seq, if lane == "main" { "main" } else { "thread" }))
            .collect();
        assert_eq!(lane_mutations, vec![(4, "main"), (5, "thread")]);
        Ok(())
    })
}

#[test]
fn forks_before_an_entry_without_modifying_the_source() -> TestResult {
    for_each_backend(|repo| {
        let source = create(repo, "source")?;
        let (root, tail) = {
            let mut source = lock_session(&source);
            let root = source.append_message("main", user_message("root"))?;
            let tail = source.append_message("main", user_message("tail"))?;
            (root, tail)
        };

        let oldest = EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        };
        let fork = repo.fork(
            "source",
            &ForkScope::Branch {
                entry_id: Some(tail.clone()),
                position: None,
            },
            CreateOptions {
                id: Some("fork".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        {
            let fork = lock_session(&fork);
            assert_eq!(entry_ids(&fork.find_entries(&oldest)?), vec![root.as_str()]);
            assert_eq!(fork.leaf_id("main")?, Some(root.clone()));
        }
        assert_eq!(lock_session(&source).leaf_id("main")?, Some(tail.clone()));

        let before_default = repo.fork(
            "source",
            &ForkScope::Branch {
                entry_id: None,
                position: Some(ForkPosition::Before),
            },
            CreateOptions {
                id: Some("before-default-target".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        {
            let before_default = lock_session(&before_default);
            assert_eq!(
                entry_ids(&before_default.find_entries(&oldest)?),
                vec![root.as_str()]
            );
            assert_eq!(before_default.leaf_id("main")?, Some(root.clone()));
        }

        let at_default = repo.fork(
            "source",
            &ForkScope::Branch {
                entry_id: None,
                position: Some(ForkPosition::At),
            },
            CreateOptions {
                id: Some("at-default-target".to_owned()),
                ..CreateOptions::default()
            },
        )?;
        {
            let at_default = lock_session(&at_default);
            assert_eq!(
                entry_ids(&at_default.find_entries(&oldest)?),
                vec![root.as_str(), tail.as_str()]
            );
            assert_eq!(at_default.leaf_id("main")?, Some(tail.clone()));
        }

        assert_code(
            repo.fork(
                "source",
                &ForkScope::Branch {
                    entry_id: Some("missing".to_owned()),
                    position: None,
                },
                CreateOptions {
                    id: Some("missing-fork".to_owned()),
                    ..CreateOptions::default()
                },
            ),
            "invalid_fork_target",
        )?;
        Ok(())
    })
}

#[test]
fn validates_the_default_fork_target() -> TestResult {
    for_each_backend(|repo| {
        let source = create(repo, "source-with-custom-leaf")?;
        lock_session(&source).append_custom("main", "not-a-message", None)?;
        assert_code(
            repo.fork(
                "source-with-custom-leaf",
                &ForkScope::default(),
                CreateOptions {
                    id: Some("fork".to_owned()),
                    ..CreateOptions::default()
                },
            ),
            "invalid_fork_target",
        )?;
        Ok(())
    })
}
