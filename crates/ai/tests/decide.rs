use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use yi_ai::decide::decide;
use yi_types::classifier::{DecisionRequest, Question};

type Res = Result<(), Box<dyn std::error::Error>>;

fn skill_question() -> DecisionRequest {
    let criteria = [
        ("land", "open and merge a forge PR"),
        ("none", "no listed method applies"),
    ]
    .into_iter()
    .map(|(label, text)| (label.to_owned(), text.to_owned()))
    .collect();
    DecisionRequest {
        state: [("message".to_owned(), serde_json::json!("land this branch"))].into(),
        questions: [(
            "skill".to_owned(),
            Question::Choice {
                instructions: "Which method does the message ask for?".to_owned(),
                criteria,
            },
        )]
        .into(),
        model: Some("english".to_owned()),
    }
}

/// Serves one connection with `status` and `body`; returns the request head it read.
fn serve_once(status: &str, body: &str) -> std::io::Result<(u16, std::thread::JoinHandle<String>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (status, body) = (status.to_owned(), body.to_owned());
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return String::new();
        };
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            head.push_str(&line);
        }
        let mut sent = vec![0; length];
        let _ = reader.read_exact(&mut sent);
        let reply = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = reader.get_mut().write_all(reply.as_bytes());
        head + &String::from_utf8_lossy(&sent)
    });
    Ok((port, handle))
}

#[test]
fn a_decision_comes_back_and_the_bearer_goes_out() -> Res {
    let answer = r#"{"answers":{"skill":{"choice":"land","answer_confidence":0.91}},"routing":{"model":"english"}}"#;
    let (port, served) = serve_once("200 OK", answer)?;
    let response = decide(
        &format!("http://127.0.0.1:{port}/"),
        Some("sekrit"),
        Duration::from_secs(5),
        &skill_question(),
    )?;
    let head = served.join().map_err(|_| "server thread")?;
    assert!(head.starts_with("POST /v1/systemone "), "{head}");
    assert!(head.contains("authorization: Bearer sekrit"), "{head}");
    assert!(head.contains("\"type\":\"choice\""), "{head}");
    let skill = response.answers.get("skill").ok_or("the skill answer")?;
    assert_eq!(skill.choice.as_deref(), Some("land"));
    Ok(())
}

#[test]
fn a_refusal_names_the_status_and_never_the_key() -> Res {
    let (port, served) = serve_once("401 Unauthorized", "{}")?;
    let error = decide(
        &format!("http://127.0.0.1:{port}"),
        Some("sekrit"),
        Duration::from_secs(5),
        &skill_question(),
    )
    .err()
    .ok_or("a 401 is an error")?;
    served.join().map_err(|_| "server thread")?;
    assert!(error.contains("HTTP 401"), "{error}");
    assert!(!error.contains("sekrit"), "{error}");
    Ok(())
}

/// A sidecar that accepts and never answers must cost the deadline, not a hung turn.
#[test]
fn a_silent_sidecar_fails_at_the_deadline() -> Res {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let started = Instant::now();
    let result = decide(
        &format!("http://127.0.0.1:{port}"),
        None,
        Duration::from_millis(300),
        &skill_question(),
    );
    drop(listener);
    assert!(result.is_err(), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    Ok(())
}

/// Incident: every journaled error from a dead sidecar named its URL twice; callers name it.
#[test]
fn a_dead_sidecar_error_is_the_cause_without_the_url_or_key() -> Res {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let error = decide(
        &format!("http://127.0.0.1:{port}"),
        Some("sekrit"),
        Duration::from_millis(300),
        &skill_question(),
    )
    .err()
    .ok_or("a closed port must fail")?;
    assert!(!error.contains("127.0.0.1"), "{error}");
    assert!(error.contains("Connection Failed"), "{error}");
    assert!(!error.contains("sekrit"), "{error}");
    Ok(())
}
