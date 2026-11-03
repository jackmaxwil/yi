//! A provider declared in `config.json`, end to end through the real binary against a loopback
//! server that answers in the OpenAI chat-completions streaming shape.

use std::error::Error;
use std::io::{BufReader, Write};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::loopback::{read_request, reply};
use crate::plan_ops::streams as text;
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

type Seen = Arc<Mutex<Vec<String>>>;

const ANSWER: &str = concat!(
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"pong\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"m1\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":1,\"total_tokens\":13}}\n\n",
    "data: [DONE]\n\n",
);

/// Answers every request with [`ANSWER`] and keeps each request whole.
fn fake_gateway() -> std::io::Result<(u16, Seen)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            let answer = reply("200 OK", "text/event-stream", ANSWER);
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
        #[expect(
            clippy::disallowed_methods,
            reason = "the journey is the spawned binary's argv, exit code, stdout and files"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        command
            .args(args)
            .env("HOME", self.dir.join("home"))
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
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
    assert!(detail.starts_with("localgw 1 model(s)"), "{row}");
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
