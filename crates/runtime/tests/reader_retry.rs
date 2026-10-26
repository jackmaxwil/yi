use crate::compaction_faux::{anthropic_sse, openrouter_stand_in};
use crate::reader_e2e::{Setup, child_rows, faux_model, kwargs, reply, wired_family};

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
) -> Result<(Arc<yi_runtime::SubagentHost>, Vec<(String, String)>), Box<dyn std::error::Error>> {
    let script = answers.iter().map(|text| reply(text)).collect();
    let family = wired_family(4, Arc::new(Mutex::new(script)), setup)?;
    let args = json!({"name": "retry", "role": "reader", "schema": schema()});
    family.host.spawn("answer".to_owned(), kwargs(args))?;
    let rows = child_rows(&family, "retry").await?;
    Ok((family.host, rows))
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
        let (host, rows) = run(Setup::default(), &[bad, next]).await?;
        assert_eq!(assistants(&rows), calls, "{bad}: {rows:?}");
        assert!(
            error.is_empty()
                || rows
                    .iter()
                    .any(|(role, text)| { role == "user" && text.contains(error) }),
            "retry error {error}: {rows:?}"
        );
        match host.result("retry", Some(&schema())) {
            Ok(value) => assert!(valid && value["json"]["answer"] == "sea"),
            Err(error) => assert!(!valid && error.contains("text, not JSON"), "{error}"),
        }
    }
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
    let (_, rows) = run(setup, &[]).await?;
    assert_eq!(assistants(&rows), 3, "{rows:?}");
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
    Ok(())
}
