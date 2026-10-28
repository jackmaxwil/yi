use std::error::Error;

use serde_json::{Value, json};
use yi_ai::auth::Secret;
use yi_ai::search::{
    OpenRouterSearch, ResultCount, SearchDomain, SearchEngine, SearchError, SearchMode,
    SearchProvider, SearchQuery, SearchRequest, SearchResponse,
};

const DEEPSEEK_REQUEST: &str =
    include_str!("fixtures/openrouter-web-search-deepseek-q0.request.json");
const DEEPSEEK_RESPONSE: &str =
    include_str!("fixtures/openrouter-web-search-deepseek-q0.response.json");
const GPT_REQUEST: &str = include_str!("fixtures/openrouter-web-search-gpt5-nano-q0.request.json");
const GPT_RESPONSE: &str =
    include_str!("fixtures/openrouter-web-search-gpt5-nano-q0.response.json");
const INSTRUCTION: &str = "Perform exactly one web search for the user's query, then answer in at most 150 words using only what the search returned, citing each claim with a markdown link to its source.";

type TestResult = Result<(), Box<dyn Error>>;

fn request(count: u8) -> Result<SearchRequest, Box<dyn Error>> {
    Ok(SearchRequest::new(
        SearchQuery::new("What changed in the latest stable Rust release?")?,
        ResultCount::new(count)?,
        Vec::new(),
        SearchMode::Auto,
    ))
}

fn call(
    request: &SearchRequest,
    status: &str,
    body: &str,
    model_id: &str,
    routing: Option<Value>,
) -> Result<(Result<SearchResponse, SearchError>, String), Box<dyn Error>> {
    let reply = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let (port, _, served) = crate::loopback_server::serve([reply])?;
    let model = crate::common::model(
        model_id,
        "openai-completions",
        "openrouter",
        &format!("http://127.0.0.1:{port}/api/v1"),
        None,
    );
    let provider = OpenRouterSearch::new(
        model,
        Some(Secret::new("sekrit".to_owned())),
        SearchEngine::new("exa")?,
        None,
        routing,
    )?;
    let response = provider.search(request);
    let requests = served.join().map_err(|_| "server thread panicked")?;
    let captured = requests
        .first()
        .ok_or("no request reached the server")?
        .clone();
    Ok((response, captured))
}

#[test]
fn the_recorded_deepseek_exchange_preserves_request_citations_usage_and_cost() -> TestResult {
    let (response, captured) = call(
        &request(5)?,
        "200 OK",
        DEEPSEEK_RESPONSE,
        "deepseek/deepseek-v4-flash",
        None,
    )?;
    assert_eq!(
        crate::loopback::json_body(&captured)?,
        serde_json::from_str::<Value>(DEEPSEEK_REQUEST)?
    );
    assert!(
        captured.contains("authorization: Bearer sekrit")
            || captured.contains("Authorization: Bearer sekrit")
    );

    let response = response?;
    let recorded: Value = serde_json::from_str(DEEPSEEK_RESPONSE)?;
    let message = &recorded["choices"][0]["message"];
    assert_eq!(
        response.answer,
        message["content"].as_str().ok_or("missing answer")?
    );
    assert_eq!(response.provider, "openrouter");
    assert_eq!(response.model, "deepseek/deepseek-v4-flash");
    assert_eq!(response.upstream.as_deref(), Some("Venice"));
    assert_eq!(response.engine.as_str(), "exa");
    assert_eq!(response.mode, SearchMode::Auto);
    assert_eq!(response.searches, 1);
    assert_eq!(response.usage.input, 4475);
    assert_eq!(response.usage.output, 437);
    assert_eq!(
        response.usage.cost.total,
        recorded["usage"]["cost"]
            .as_number()
            .ok_or("missing cost")?
            .clone()
    );
    let annotations = message["annotations"]
        .as_array()
        .ok_or("missing annotations")?;
    assert_eq!(response.citations.len(), annotations.len());
    for (citation, annotation) in response.citations.iter().zip(annotations) {
        let source = &annotation["url_citation"];
        assert_eq!(
            citation.title,
            source["title"].as_str().ok_or("missing title")?
        );
        assert_eq!(
            citation.url.as_str(),
            source["url"].as_str().ok_or("missing URL")?
        );
        assert_eq!(
            citation.excerpt,
            source["content"].as_str().ok_or("missing excerpt")?
        );
    }
    Ok(())
}

#[test]
fn the_recorded_gpt_response_and_input_output_usage_aliases_parse() -> TestResult {
    let (response, captured) = call(
        &request(5)?,
        "200 OK",
        GPT_RESPONSE,
        "openai/gpt-5-nano",
        None,
    )?;
    assert_eq!(
        crate::loopback::json_body(&captured)?,
        serde_json::from_str::<Value>(GPT_REQUEST)?
    );
    let response = response?;
    assert_eq!(response.citations.len(), 5);
    assert_eq!(response.upstream.as_deref(), Some("Azure"));
    assert_eq!(response.usage.input, 4026);
    assert_eq!(response.usage.output, 5192);
    assert_eq!(response.searches, 1);

    let mut aliased: Value = serde_json::from_str(GPT_RESPONSE)?;
    let usage = aliased["usage"].as_object_mut().ok_or("missing usage")?;
    let input = usage
        .remove("prompt_tokens")
        .ok_or("missing prompt tokens")?;
    let output = usage
        .remove("completion_tokens")
        .ok_or("missing completion tokens")?;
    usage.insert("input_tokens".to_owned(), input);
    usage.insert("output_tokens".to_owned(), output);
    let (response, _) = call(
        &request(5)?,
        "200 OK",
        &aliased.to_string(),
        "openai/gpt-5-nano",
        None,
    )?;
    let response = response?;
    assert_eq!(response.usage.input, 4026);
    assert_eq!(response.usage.output, 5192);
    Ok(())
}

#[test]
fn count_limits_domains_mode_and_routing_reach_the_exact_wire_fields() -> TestResult {
    for count in [1, 25] {
        let mut input = request(count)?;
        input.domains = vec![SearchDomain::new("doc.rust-lang.org")?];
        input.mode = SearchMode::DeepReasoning;
        let (_, captured) = call(
            &input,
            "200 OK",
            DEEPSEEK_RESPONSE,
            "deepseek/deepseek-v4-flash",
            Some(json!({"order": ["Venice"]})),
        )?;
        let body = crate::loopback::json_body(&captured)?;
        let parameters = &body["tools"][0]["parameters"];
        assert_eq!(parameters["max_results"], count);
        assert_eq!(parameters["max_total_results"], count);
        assert_eq!(parameters["max_uses"], 1);
        assert_eq!(parameters["allowed_domains"], json!(["doc.rust-lang.org"]));
        assert_eq!(parameters["mode"], "deep-reasoning");
        assert!(body.get("provider").is_none());
        assert_eq!(body["messages"][0]["content"], INSTRUCTION);
        assert_eq!(body["stream"], false);
    }
    assert!(matches!(
        ResultCount::new(0),
        Err(SearchError::InvalidInput(_))
    ));
    assert!(matches!(
        ResultCount::new(26),
        Err(SearchError::InvalidInput(_))
    ));
    assert!(SearchQuery::new("  ").is_err());
    Ok(())
}

#[test]
fn malformed_json_is_a_protocol_error() -> TestResult {
    let (response, _) = call(
        &request(5)?,
        "200 OK",
        "not json",
        "deepseek/deepseek-v4-flash",
        None,
    )?;
    assert!(matches!(response, Err(SearchError::Protocol(_))));
    Ok(())
}

#[test]
fn http_failures_keep_their_status_and_hide_the_credential() -> TestResult {
    for (status, code) in [("400 Bad Request", 400), ("500 Internal Server Error", 500)] {
        let (response, _) = call(
            &request(5)?,
            status,
            r#"{"error":{"message":"bad engine; do not echo sekrit"}}"#,
            "deepseek/deepseek-v4-flash",
            None,
        )?;
        match response {
            Err(SearchError::Http { status, message }) => {
                assert_eq!(status, code);
                assert!(!message.contains("bad engine"));
                assert!(!message.contains("sekrit"));
            }
            other => return Err(format!("expected HTTP {code}, got {other:?}").into()),
        }
    }
    Ok(())
}

#[test]
fn zero_annotations_is_distinct_from_a_malformed_response_and_keeps_its_cost() -> TestResult {
    let mut body: Value = serde_json::from_str(DEEPSEEK_RESPONSE)?;
    body["choices"][0]["message"]["annotations"] = json!([]);
    let (response, _) = call(
        &request(5)?,
        "200 OK",
        &body.to_string(),
        "deepseek/deepseek-v4-flash",
        None,
    )?;
    let Err(SearchError::Billed { error, usage }) = response else {
        return Err("a paid response without citations did not retain its usage".into());
    };
    assert!(matches!(*error, SearchError::NoCitations));
    assert_eq!(usage.cost.total.as_f64(), Some(0.007516407500000001));
    Ok(())
}

#[test]
fn missing_credentials_are_refused() -> TestResult {
    let model = crate::common::model(
        "deepseek/deepseek-v4-flash",
        "openai-completions",
        "openrouter",
        "http://127.0.0.1:1/api/v1",
        None,
    );
    assert!(matches!(
        OpenRouterSearch::new(model, None, SearchEngine::new("exa")?, None, None),
        Err(SearchError::MissingCredential)
    ));
    Ok(())
}
