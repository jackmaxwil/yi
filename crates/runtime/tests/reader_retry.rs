use crate::compaction_faux::{anthropic_sse, openrouter_stand_in};
use crate::reader_e2e::{Family, Setup, child_rows, faux_model, kwargs, reply, wired_family};

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

const GOOD: &str = "{\"answer\":\"sea\"}";
const FENCE: &str = "Here is the result:\n```json\n{\"answer\":\"sea\"}\n```";
const MISSING: &str = "missing required property answer";

fn schema() -> Value {
    json!({"type": "object", "properties": {"answer": {"type": "string"}},
        "required": ["answer"], "additionalProperties": false})
}

async fn run(
    setup: Setup,
    answers: &[&str],
) -> Result<(Family, Vec<(String, String)>), Box<dyn std::error::Error>> {
    let script = answers.iter().map(|text| reply(text)).collect();
    let family = wired_family(4, Arc::new(Mutex::new(script)), setup)?;
    let args = json!({"name": "retry", "role": "reader", "schema": schema()});
    family.host.spawn("answer".to_owned(), kwargs(args))?;
    let rows = child_rows(&family, "retry").await?;
    Ok((family, rows))
}

fn assistants(rows: &[(String, String)]) -> usize {
    rows.iter().filter(|(role, _)| role == "assistant").count()
}

/// #989's fence, a K3.1 reader's missing field, #992's empty last word, then prose twice.
#[tokio::test]
async fn schema_failure_gets_one_retry_and_a_second_failure_is_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for (bad, next, calls, error, valid) in [
        (FENCE, "", 1, "", true),
        ("{}", GOOD, 2, MISSING, true),
        ("", GOOD, 2, "answer contains no JSON value", true),
        ("prose", "still prose", 2, "", false),
    ] {
        let (family, rows) = run(Setup::default(), &[bad, next]).await?;
        assert_eq!(assistants(&rows), calls, "{bad}: {rows:?}");
        assert!(
            error.is_empty()
                || rows
                    .iter()
                    .any(|(role, text)| { role == "user" && text.contains(error) }),
            "retry error {error}: {rows:?}"
        );
        match family.host.result("retry", Some(&schema())) {
            Ok(value) => assert!(valid && value["json"]["answer"] == "sea"),
            Err(error) => assert!(!valid && error.contains("text, not JSON"), "{error}"),
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::disallowed_methods,
    reason = "a FIFO holds the real read tool open while ordinary mail arrives"
)]
async fn ordinary_mail_does_not_reopen_the_original_schema_request()
-> Result<(), Box<dyn std::error::Error>> {
    let fifo = std::env::temp_dir().join(format!("yi-reader-retry-{}", std::process::id()));
    let _ = std::fs::remove_file(&fifo);
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()?
            .success()
    );
    let followup = yi_ai::faux::faux_assistant_message(
        vec![
            yi_ai::faux::faux_text(GOOD),
            yi_ai::faux::faux_tool_call("hold", "read", kwargs(json!({"path": fifo}))),
        ],
        yi_types::message::StopReason::ToolUse,
    );
    let script = vec![reply(GOOD), followup, reply("mail prose"), reply(GOOD)];
    let family = wired_family(4, Arc::new(Mutex::new(script)), Setup::default())?;
    let args = json!({"name": "retry", "role": "reader", "schema": schema()});
    family.host.spawn("answer".to_owned(), kwargs(args))?;
    let initial = child_rows(&family, "retry").await?;
    assert_eq!(assistants(&initial), 1, "{initial:?}");
    let session = family.host.children_view().pop().ok_or("no child")?.session;
    family
        .host
        .route("parent", "retry", "another request", true)?;
    for _ in 0..400 {
        if session.status() == yi_runtime::Status::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let sent = family
        .host
        .route("parent", "retry", "ordinary mail", false)?;
    assert_eq!(sent["receipts"][0]["state"], "queued", "{sent:?}");
    std::fs::write(&fifo, "held\n")?;
    let rows = child_rows(&family, "retry").await?;
    assert_eq!(assistants(&rows), 3, "{rows:?}");
    assert!(
        !rows
            .iter()
            .any(|(role, text)| { role == "user" && text.contains("schema retry:") }),
        "{rows:?}"
    );
    let error = family
        .host
        .result("retry", Some(&schema()))
        .err()
        .ok_or_else(|| format!("mail prose unexpectedly satisfied the schema: {rows:?}"))?;
    assert!(error.contains("mail prose"), "{error}");
    Ok(())
}

/// A recorded reader turn with one read call, then its missing-field reply (#948's wire shape).
#[tokio::test(flavor = "multi_thread")]
async fn an_anthropic_retry_over_tool_blocks_is_one_cold_text_request()
-> Result<(), Box<dyn std::error::Error>> {
    let call = json!({"type": "tool_use", "id": "toolu_1", "name": "read"});
    let (port, served) = openrouter_stand_in(vec![
        anthropic_sse(&call, "tool_use"),
        anthropic_sse(&json!({"type": "text", "text": "{}"}), "end_turn"),
        anthropic_sse(&json!({"type": "text", "text": GOOD}), "end_turn"),
        anthropic_sse(&json!({"type": "text", "text": GOOD}), "end_turn"),
    ])?;
    let mut model = faux_model(200_000);
    model.id = "claude-haiku-4-5".into();
    model.api = "anthropic-messages".into();
    model.provider = "anthropic".into();
    let setup = Setup {
        provider: Some(crate::upstream::stream(&model, port, false)?),
        model: Some(model),
        ..Setup::default()
    };
    let (family, rows) = run(setup, &[]).await?;
    assert_eq!(assistants(&rows), 3, "{rows:?}");
    family.host.route("parent", "retry", "answer again", true)?;
    let rows = child_rows(&family, "retry").await?;
    assert_eq!(assistants(&rows), 4, "{rows:?}");
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let cold: Value = serde_json::from_str(bodies.get(2).ok_or("no cold request")?)?;
    let sent = cold["messages"].as_array().ok_or("no messages")?;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["role"], "user");
    assert!(cold.get("tools").is_none());
    assert!(cold["output_config"]["format"]["type"] == "json_schema");
    let text = sent[0]["content"].to_string();
    assert!(text.contains("read("));
    assert!(text.contains(MISSING));
    assert!(!text.contains("tool_use") && !text.contains("tool_result"));
    let follow_up: Value = serde_json::from_str(bodies.get(3).ok_or("no follow-up")?)?;
    assert!(follow_up.get("tools").is_some(), "{follow_up}");
    assert!(
        follow_up["messages"]
            .as_array()
            .is_some_and(|messages| messages.len() > 1),
        "{follow_up}"
    );
    assert!(follow_up.to_string().contains("tool_use"), "{follow_up}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_restores_tools_and_gets_its_own_repair()
-> Result<(), Box<dyn std::error::Error>> {
    let follow_up_call = json!({"type": "tool_use", "id": "toolu_2", "name": "read"});
    let (port, served) = openrouter_stand_in(vec![
        anthropic_sse(&json!({"type": "text", "text": "{}"}), "end_turn"),
        anthropic_sse(&json!({"type": "text", "text": GOOD}), "end_turn"),
        anthropic_sse(&follow_up_call, "tool_use"),
        anthropic_sse(&json!({"type": "text", "text": "{}"}), "end_turn"),
        anthropic_sse(&json!({"type": "text", "text": GOOD}), "end_turn"),
    ])?;
    let mut model = faux_model(200_000);
    model.id = "claude-haiku-4-5".into();
    model.api = "anthropic-messages".into();
    model.provider = "anthropic".into();
    let setup = Setup {
        provider: Some(crate::upstream::stream(&model, port, false)?),
        model: Some(model),
        ..Setup::default()
    };
    let (family, rows) = run(setup, &[]).await?;
    assert_eq!(assistants(&rows), 2, "{rows:?}");
    family.host.route("parent", "retry", "answer again", true)?;
    let rows = child_rows(&family, "retry").await?;
    assert_eq!(assistants(&rows), 5, "{rows:?}");
    let bodies = served.join().map_err(|_| "the stand-in panicked")?;
    let requests: Vec<Value> = bodies
        .iter()
        .map(|body| serde_json::from_str(body))
        .collect::<Result<_, _>>()?;
    let warm = requests.get(1).ok_or("no initial repair")?;
    assert!(
        warm["messages"]
            .as_array()
            .is_some_and(|messages| messages.len() > 1)
    );
    assert!(warm.get("tools").is_none());
    let follow_up = requests.get(2).ok_or("no follow-up request")?;
    assert!(follow_up.get("tools").is_some(), "{follow_up}");
    let cold = requests.get(4).ok_or("no follow-up repair")?;
    assert_eq!(cold["messages"].as_array().map(Vec::len), Some(1));
    assert!(cold.get("tools").is_none());
    assert_eq!(
        family.host.result("retry", Some(&schema()))?["json"]["answer"],
        "sea"
    );
    Ok(())
}
