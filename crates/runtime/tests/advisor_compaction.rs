//! E2 (§16 `CompactionCheck`): a compaction replaces the primary's view, and the
//! advisor is the only thing that can say what the replacement dropped.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_context::{Settings, Tokens};
use yi_loop::ExecutionMode;
use yi_runtime::advisor::{AdvisorConfig, AdvisorRuntime, digest, note_last_compaction};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

const CONSTRAINT: &str = "Never touch vendored/lock.json (SENTINEL-K7QX).";

fn faux_model(context_window: u64) -> Model {
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
        context_window,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn reply_with_usage(text: &str, input: i64, total: i64) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = input;
        usage.output = total.saturating_sub(input);
        usage.total_tokens = total;
    }
    message
}

fn compaction_summary(summary: &str) -> AgentMessage {
    AgentMessage::CompactionSummary {
        summary: summary.to_owned(),
        tokens_before: 0,
        timestamp: 0,
    }
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
}

fn tool_result(name: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "t1".to_owned(),
        tool_name: name.to_owned(),
        content: vec![Content::Text {
            text: "ok".to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

fn runtime(config: AdvisorConfig) -> Arc<AdvisorRuntime> {
    Arc::new(AdvisorRuntime::new(config, Arc::new(|_message| {}), None))
}

/// A real faux compaction against a real store, so the seam reads the entry the
/// compactor actually wrote rather than one the test hand-built.
async fn compacted_store(id: &str) -> Result<(Scratch, yi_session::SharedSession), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-advisor-compact-{id}"))?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-advisor-compact");
    let store = repo.create(CreateOptions {
        id: Some(id.to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        reply_with_usage(&format!("long body {}", "y".repeat(400)), 100, 5_000),
        faux_assistant_message(
            vec![faux_text(
                "## Goal\nPort the parser. Files touched: src/parse.rs",
            )],
            StopReason::Stop,
        ),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(2_000),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction_with(Settings {
        enabled: true,
        reserve_tokens: Tokens(1_000),
        keep_recent_tokens: Tokens(10),
    });
    session.attach_store(Arc::clone(&store))?;
    session.prompt(&format!("{CONSTRAINT} Start the port."))?;
    session.wait_idle().await;
    if !session.compact_now().await {
        return Err("the fixture must actually compact".into());
    }
    Ok((root, store))
}

#[test]
fn a_compaction_summary_reaches_the_digest_as_a_pullable_line() -> TestResult {
    let message = compaction_summary(&format!("head fact {}", "z".repeat(400)));
    let line = digest::digest_line(
        &digest::LogItem {
            id: "m4",
            message: &message,
        },
        digest::DEFAULT_USER_BUDGET,
        digest::DEFAULT_PROSE_BUDGET,
    )
    .ok_or("a compaction summary must render a digest line")?;
    assert!(
        line.starts_with("m4 compaction: head fact"),
        "the judge must see the replacement summary, id-addressed: {line}"
    );
    assert!(
        line.contains("[pull m4]"),
        "an elided summary must carry its pull handle: {line}"
    );
    Ok(())
}

#[test]
fn a_view_prefixed_summary_shows_the_goal_in_the_digest_head() -> TestResult {
    let summary = "<yi_compact_view>\n[Kernel] Note: the IPython kernel keeps running after this summary\n</yi_compact_view>\n## Goal\nPort the parser. Files touched: src/parse.rs";
    let line = digest::digest_line(
        &digest::LogItem {
            id: "m2",
            message: &compaction_summary(summary),
        },
        digest::DEFAULT_USER_BUDGET,
        digest::DEFAULT_PROSE_BUDGET,
    )
    .ok_or("a compaction summary must render a digest line")?;
    assert!(
        line.contains("Port the parser"),
        "the 200-char head must skip the view wrapper so the judge sees Goal/Next: {line}"
    );
    assert!(
        !line.contains("<yi_compact_view>"),
        "the wrapper must not occupy the head when Goal/Next follows it: {line}"
    );
    Ok(())
}

#[tokio::test]
async fn a_compaction_hands_the_judge_the_summary_beside_the_directives() -> TestResult {
    let (_root, store) = compacted_store("advisor-compact-one").await?;
    let advisor = runtime(AdvisorConfig::default());
    // The directive was stated before the compaction: it is the ground truth
    // the judge audits the replacement summary against (§16).
    assert!(advisor.observe(&user(CONSTRAINT), 0).is_none());

    note_last_compaction(Some(&advisor), Some(&store));
    let chunk = advisor
        .observe(&tool_result("bash"), 2)
        .ok_or("a compaction must request the next boundary's review")?;

    // The `compaction:` prefix is the judge's handle for the line; the context
    // note must not wear it, or the two are indistinguishable in the chunk.
    let compaction_line = chunk
        .lines()
        .find(|line| line.contains(" compaction: "))
        .ok_or_else(|| format!("no compaction line in the digest chunk:\n{chunk}"))?;
    assert!(
        compaction_line.contains("Port the parser"),
        "the line must carry the summary the compactor wrote: {compaction_line}"
    );
    assert!(
        chunk.contains("SENTINEL-K7QX"),
        "the directives panel must ride the same chunk, or nothing grounds the audit:\n{chunk}"
    );

    let pull_id = compaction_line
        .split_whitespace()
        .next()
        .ok_or("the compaction line must start with its entry id")?;
    let full = advisor
        .transcript(pull_id)
        .ok_or_else(|| format!("transcript({pull_id}) must resolve the compaction summary"))?;
    assert!(
        full.contains("src/parse.rs"),
        "pull must return the whole summary, not the head: {full}"
    );
    Ok(())
}

#[tokio::test]
async fn a_compaction_review_is_budget_gated() -> TestResult {
    let (_root, store) = compacted_store("advisor-compact-two").await?;
    let advisor = runtime(AdvisorConfig {
        tokens_per_hour: Some(100),
        ..AdvisorConfig::default()
    });
    advisor.record_spend(0, 100);

    note_last_compaction(Some(&advisor), Some(&store));
    assert!(
        advisor.observe(&tool_result("bash"), 2).is_none(),
        "an exhausted hourly budget must swallow the compaction review, not overspend on it"
    );
    Ok(())
}

#[test]
fn a_storeless_compaction_audit_is_skipped_not_faked() -> TestResult {
    let root = Scratch::new("yi-advisor-empty")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-advisor-empty");
    let store = repo.create(CreateOptions::default())?;
    let advisor = runtime(AdvisorConfig::default());

    note_last_compaction(Some(&advisor), Some(&store));
    assert!(
        advisor.observe(&tool_result("bash"), 2).is_none(),
        "a session that never compacted must not trigger a compaction review"
    );
    Ok(())
}
