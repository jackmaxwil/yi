//! A provider declared in `config.json`, end to end through the real binary against a loopback
//! server that answers in the OpenAI chat-completions streaming shape.

use std::error::Error;
use std::io::{BufReader, Write};
use std::process::Output;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::loopback::{read_request, reply};
use crate::plan_ops::streams as text;
use crate::scratch::Scratch;
use crate::yi_bin::yi_command;

type TestResult = Result<(), Box<dyn Error>>;

type Seen = Arc<Mutex<Vec<String>>>;

const ANSWER: &str = concat!(
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"pong\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":1,\"total_tokens\":13}}\n\n",
    "data: [DONE]\n\n",
);

const MESSAGES: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"m1\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const RESPONSES: &str = concat!(
    "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n",
    "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
);

const DISCOVERY: &str = r#"{"data":[{"id":"m2","object":"model","context_window":64000,"max_tokens":16000,"capabilities":{"image_input":true}}]}"#;

/// Answers each request in the shape of the wire its path names and keeps each request whole.
fn fake_gateway() -> std::io::Result<(u16, Seen)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            let line = request.lines().next().unwrap_or_default();
            let (content_type, body) = if line.contains("/models ") {
                ("application/json", DISCOVERY)
            } else if line.contains("/messages ") {
                ("text/event-stream", MESSAGES)
            } else if line.contains("/responses ") {
                ("text/event-stream", RESPONSES)
            } else {
                ("text/event-stream", ANSWER)
            };
            let answer = reply("200 OK", content_type, body);
            let _ = reader.get_mut().write_all(answer.as_bytes());
            if let Ok(mut all) = kept.lock() {
                all.push(request);
            }
        }
    });
    Ok((port, seen))
}

struct Home {
    dir: Scratch,
}

impl Home {
    fn new(tag: &str, providers: &Value) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-custom-{tag}"))?;
        let home = dir.home()?;
        std::fs::create_dir_all(dir.join("project"))?;
        let config = json!({"kernel": {"prewarm": false}, "providers": providers});
        std::fs::write(home.join(".yi/config.json"), config.to_string())?;
        Ok(Self { dir })
    }

    fn yi(&self, args: &[&str], env: &[(&str, &str)]) -> std::io::Result<Output> {
        let mut command = yi_command(&self.dir.join("home"));
        command
            .args(args)
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("LOCALGW_API_KEY")
            .envs(env.iter().copied())
            .current_dir(self.dir.join("project"))
            .output()
    }
}

fn localgw(port: u16, key: Value) -> Value {
    let mut provider = json!({
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "models": [{"id": "m1", "name": "Model One", "contextWindow": 200000, "maxTokens": 32000}],
    });
    if let (Some(provider), Some(key)) = (provider.as_object_mut(), key.as_object()) {
        provider.extend(key.clone());
    }
    json!({ "localgw": provider })
}

/// The `providers` row of `yi doctor --json` run with `env`.
fn providers_row(home: &Home, env: &[(&str, &str)]) -> Result<Value, Box<dyn Error>> {
    let mut env = env.to_vec();
    env.push(("YI_KERNEL_PYTHON", "/nonexistent/python3"));
    let doctor = home.yi(&["doctor", "--json"], &env)?;
    let rows: Value = serde_json::from_slice(&doctor.stdout)?;
    let row = (rows.as_array().into_iter().flatten())
        .find(|row| row["name"] == "providers")
        .ok_or_else(|| format!("no providers row: {rows}"))?;
    Ok(row.clone())
}

/// The request reaches the declared base URL's `/chat/completions` with the env key as the
/// bearer, the declared id as the model, and plain `max_tokens` a loopback endpoint accepts.
#[test]
fn a_declared_provider_streams_through_its_own_base_url_and_key() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let home = Home::new(
        "ask",
        &localgw(port, json!({"apiKeyEnv": "LOCALGW_API_KEY"})),
    )?;
    let ask = home.yi(
        &[
            "ask",
            "--here",
            "--model",
            "localgw/m1",
            "Reply with exactly: pong",
        ],
        &[("LOCALGW_API_KEY", "sk-localgw-test")],
    )?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    assert!(
        String::from_utf8_lossy(&ask.stdout).contains("pong"),
        "{}",
        text(&ask)
    );
    let requests = seen.lock().map_err(|_| "fake lock")?.clone();
    let first = requests.first().ok_or("the server saw no request")?;
    assert!(first.starts_with("POST /v1/chat/completions "), "{first}");
    assert!(
        first
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer sk-localgw-test")),
        "{first}"
    );
    let body: Value = serde_json::from_str(first.rsplit("\r\n").next().unwrap_or_default())?;
    assert_eq!(body["model"], "m1");
    assert!(body["max_tokens"].is_u64(), "{body}");
    assert!(body.get("max_completion_tokens").is_none(), "{body}");
    assert!(body.get("store").is_none(), "{body}");
    let row = providers_row(&home, &[("LOCALGW_API_KEY", "sk-localgw-test")])?;
    assert_eq!(row["status"], "ok", "{row}");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(detail.ends_with("key from LOCALGW_API_KEY"), "{row}");
    Ok(())
}

#[test]
fn a_provider_without_models_discovers_them_on_first_use() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let providers = json!({"localgw": {
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover", &providers)?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m2", "hi"], &[])?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    assert!(
        String::from_utf8_lossy(&ask.stdout).contains("pong"),
        "{}",
        text(&ask)
    );
    let requests = seen.lock().map_err(|_| "fake lock")?.clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    let discovery = requests.first().ok_or("no discovery request")?;
    assert!(discovery.starts_with("GET /v1/models "), "{discovery}");
    assert!(
        discovery
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer sk-placeholder")),
        "{discovery}"
    );
    let completion = requests.get(1).ok_or("no completion request")?;
    let body: Value = serde_json::from_str(completion.rsplit("\r\n").next().unwrap_or_default())?;
    assert_eq!(body["model"], "m2");
    assert_eq!(body["max_tokens"], 16000);
    let row = providers_row(&home, &[("YI_KERNEL_PYTHON", "/nonexistent/python3")])?;
    assert_eq!(row["status"], "ok", "{row}");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("1 dynamic models"), "{row}");
    Ok(())
}

#[test]
fn a_down_models_endpoint_is_named_instead_of_an_unknown_model() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let _ = read_request(&mut reader);
            let answer = reply("500 Internal Server Error", "application/json", "{}");
            let _ = reader.get_mut().write_all(answer.as_bytes());
        }
    });
    let providers = json!({"localgw": {
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover-down", &providers)?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m2", "hi"], &[])?;
    assert_ne!(ask.status.code(), Some(0));
    let err = text(&ask);
    assert!(err.contains("/v1/models"), "{err}");
    assert!(!err.contains("unknown model"), "{err}");
    let row = providers_row(&home, &[("YI_KERNEL_PYTHON", "/nonexistent/python3")])?;
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("/v1/models"), "{row}");
    assert_ne!(row["status"], "ok", "{row}");
    Ok(())
}

#[test]
fn a_model_list_includes_a_discovered_model() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let providers = json!({"localgw": {
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover-list", &providers)?;
    let mut child = yi_command(&home.dir.join("home"))
        .args(["rpc", "--model", "faux/faux-1"])
        .env_remove("HTTPS_PROXY")
        .env_remove("HTTP_PROXY")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"{\"id\":\"m\",\"type\":\"get_available_models\"}\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let frames = String::from_utf8_lossy(&output.stdout);
    let frame = frames
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|frame| frame["id"] == "m")
        .ok_or_else(|| format!("no models frame: {frames}"))?;
    let models = frame["data"]["models"].as_array().ok_or("no models")?;
    assert!(
        models
            .iter()
            .any(|model| model["provider"] == "localgw" && model["id"] == "m2"),
        "{frame}"
    );
    let requests = seen.lock().map_err(|_| "fake lock")?.clone();
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("GET /v1/models ")),
        "{requests:?}"
    );
    Ok(())
}

#[test]
fn a_model_list_without_a_data_array_is_named() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let _ = read_request(&mut reader);
            let answer = reply("200 OK", "application/json", r#"{"models":[{"id":"m2"}]}"#);
            let _ = reader.get_mut().write_all(answer.as_bytes());
        }
    });
    let providers = json!({"localgw": {
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover-shape", &providers)?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m2", "hi"], &[])?;
    assert_ne!(ask.status.code(), Some(0));
    let err = text(&ask);
    assert!(err.contains("no data array"), "{err}");
    assert!(!err.contains("unknown model"), "{err}");
    Ok(())
}

#[test]
fn a_discovery_get_goes_through_https_proxy() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let providers = json!({"localgw": {
        "baseUrl": "http://127.0.0.1:9/v1",
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover-proxy-ok", &providers)?;
    let proxy = format!("http://127.0.0.1:{port}");
    let ask = home.yi(
        &["ask", "--here", "--model", "localgw/m2", "hi"],
        &[("HTTPS_PROXY", &proxy)],
    )?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    let requests = seen.lock().map_err(|_| "fake lock")?.clone();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("/v1/models")),
        "{requests:?}"
    );
    Ok(())
}

#[test]
fn an_undialable_proxy_is_named_before_discovery() -> TestResult {
    let providers = json!({"localgw": {
        "baseUrl": "http://127.0.0.1:9/v1",
        "apiKey": "sk-placeholder", "models": []}});
    let home = Home::new("discover-proxy", &providers)?;
    let ask = home.yi(
        &["ask", "--here", "--model", "localgw/m2", "hi"],
        &[("HTTPS_PROXY", "socks5://127.0.0.1:1")],
    )?;
    assert_ne!(ask.status.code(), Some(0));
    let err = text(&ask);
    assert!(err.contains("SOCKS proxies are not supported"), "{err}");
    assert!(!err.contains("unknown model"), "{err}");
    Ok(())
}

/// Dies with the request sent to `/chat/completions` whatever `api` says, or a bearer-only Messages
/// gateway answering 401 because the key went as `x-api-key`.
#[test]
fn a_declared_api_picks_the_wire_and_its_key_is_the_bearer() -> TestResult {
    for (api, base, path) in [
        ("anthropic-messages", "", "POST /v1/messages "),
        ("openai-responses", "/v1", "POST /v1/responses "),
    ] {
        let (port, seen) = fake_gateway()?;
        let providers = json!({"localgw": {
            "baseUrl": format!("http://127.0.0.1:{port}{base}"), "api": api,
            "apiKey": "sk-placeholder", "models": [{"id": "m1"}]}});
        let home = Home::new(&format!("api-{api}"), &providers)?;
        let ask = home.yi(&["ask", "--here", "--model", "localgw/m1", "ping"], &[])?;
        assert_eq!(ask.status.code(), Some(0), "{api}: {}", text(&ask));
        let stdout = String::from_utf8_lossy(&ask.stdout);
        assert!(stdout.contains("pong"), "{api}: {}", text(&ask));
        let requests = seen.lock().map_err(|_| "fake lock")?.clone();
        let first = requests.first().ok_or("the server saw no request")?;
        assert!(first.starts_with(path), "{api}: {first}");
        let header = |name: &str| {
            first
                .lines()
                .any(|line| line.to_ascii_lowercase().starts_with(name))
        };
        assert!(
            first
                .lines()
                .any(|line| line.eq_ignore_ascii_case("authorization: Bearer sk-placeholder")),
            "{api}: {first}"
        );
        assert!(!header("x-api-key:"), "{api}: {first}");
        assert_eq!(header("anthropic-version:"), api == "anthropic-messages");
    }
    Ok(())
}

/// A literal `apiKey` is the bearer when no variable names one.
#[test]
fn a_literal_api_key_is_the_bearer() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let home = Home::new(
        "literal",
        &localgw(port, json!({"apiKey": "sk-placeholder"})),
    )?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m1", "hi"], &[])?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    let requests = seen.lock().map_err(|_| "fake lock")?.clone();
    let first = requests.first().ok_or("the server saw no request")?;
    assert!(
        first
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer sk-placeholder")),
        "{first}"
    );
    let row = providers_row(&home, &[])?;
    assert_eq!(row["status"], "ok", "{row}");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(detail.starts_with("localgw 1 model at"), "{row}");
    assert!(detail.contains(" over openai-completions, "), "{row}");
    assert!(detail.ends_with("key from apiKey"), "{row}");
    Ok(())
}

/// No key anywhere is refused before any request, naming the variable that would supply one;
/// `yi login` has no profile for a custom id, so it is not offered.
#[test]
fn a_declared_provider_with_no_key_is_refused_before_any_request() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let home = Home::new(
        "nokey",
        &localgw(port, json!({"apiKeyEnv": "LOCALGW_API_KEY"})),
    )?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m1", "hi"], &[])?;
    assert_eq!(ask.status.code(), Some(4), "{}", text(&ask));
    let refusal = "no credential for provider localgw (export LOCALGW_API_KEY)";
    assert!(text(&ask).contains(refusal), "{}", text(&ask));
    assert!(seen.lock().map_err(|_| "fake lock")?.is_empty());
    let row = providers_row(&home, &[])?;
    assert_eq!(row["status"], "fail", "{row}");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(detail.ends_with("no key (export LOCALGW_API_KEY)"), "{row}");
    Ok(())
}

/// A key only the yi-oauth store holds names the store, whose file names sanitize the id.
#[test]
fn a_stored_key_names_the_store_it_came_from() -> TestResult {
    let home = Home::new("stored", &localgw(1, json!({})))?;
    let tokens = home.dir.join("home/.yi/providers/tokens");
    std::fs::create_dir_all(&tokens)?;
    std::fs::write(
        tokens.join("localgw.json"),
        json!({"version": 1, "kind": "key", "access": "sk-stored"}).to_string(),
    )?;
    let row = providers_row(&home, &[])?;
    assert_eq!(row["status"], "ok", "{row}");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(
        detail.ends_with("key from the yi-oauth store in ~/.yi/providers/tokens"),
        "{row}"
    );
    Ok(())
}

/// An id a catalog provider or a reserved one (`faux`, the script) holds is refused at load.
#[test]
fn a_custom_provider_named_like_a_built_in_one_is_refused_at_load() -> TestResult {
    let mut providers = localgw(1, json!({"apiKey": "x"}));
    let declared = providers
        .as_object_mut()
        .and_then(|map| map.remove("localgw"))
        .ok_or("no localgw")?;
    for taken in ["openai", "faux"] {
        let home = Home::new(&format!("collide-{taken}"), &json!({ taken: declared }))?;
        let model = format!("{taken}/m1");
        let ask = home.yi(&["ask", "--here", "--model", &model, "hi"], &[])?;
        assert_eq!(ask.status.code(), Some(2), "{taken}: {}", text(&ask));
        assert!(
            text(&ask).contains(&format!("providers: `{taken}` is a built-in provider id")),
            "{}",
            text(&ask)
        );
    }
    Ok(())
}

/// A catalog cache that claims the custom id never replaces or drops what config declared.
#[test]
fn a_catalog_cache_leaves_the_declared_models_in_place() -> TestResult {
    let (port, seen) = fake_gateway()?;
    let home = Home::new("cache", &localgw(port, json!({"apiKey": "sk-placeholder"})))?;
    let catalog = home.dir.join("home/.yi/catalog");
    std::fs::create_dir_all(&catalog)?;
    let stolen = json!({"schema": 2, "openai-completions": {"m1": {
        "id": "m1", "name": "elsewhere", "api": "openai-completions", "provider": "localgw",
        "baseUrl": "http://127.0.0.1:9/v1", "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000, "maxTokens": 256}}});
    std::fs::write(catalog.join("openrouter.json"), stolen.to_string())?;
    let ask = home.yi(&["ask", "--here", "--model", "localgw/m1", "hi"], &[])?;
    assert_eq!(ask.status.code(), Some(0), "{}", text(&ask));
    assert_eq!(seen.lock().map_err(|_| "fake lock")?.len(), 1);
    Ok(())
}
