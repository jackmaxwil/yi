use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yi_runtime::classifier::{Deliver, Record, Sidecar, SkillClassifier};
use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope};
use yi_types::classifier::ClassifyRecord;
use yi_types::message::{AgentMessage, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

const LAND: &str = r#"{"answers":{"skill":{"choice":"land","answer_confidence":0.91}},"routing":{"model":"english"}}"#;

fn skill(name: &str, needle: &str) -> RuleDoc {
    RuleDoc {
        name: name.to_owned(),
        body: format!("skill://{name}"),
        path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
        needles: vec![needle.to_owned(), format!("${name}")],
        scope: RuleScope::Text,
        gap: RuleGap::Once,
        mode: RuleMode::Remind,
        paths: Vec::new(),
        after: 1,
    }
}

/// Answers `bodies.len()` connections in order; hands back each request body it read.
fn sidecar(
    bodies: Vec<&'static str>,
) -> std::io::Result<(u16, std::thread::JoinHandle<Vec<String>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in bodies {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut sent = vec![0; length];
            let _ = reader.read_exact(&mut sent);
            seen.push(String::from_utf8_lossy(&sent).into_owned());
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = reader.get_mut().write_all(reply.as_bytes());
        }
        seen
    });
    Ok((port, handle))
}

struct Rig {
    engine: RuleEngine,
    records: Receiver<ClassifyRecord>,
    delivered: Arc<Mutex<Vec<String>>>,
}

fn rig(url: String, threshold: Option<f64>) -> Result<Rig, Box<dyn Error>> {
    let engine = RuleEngine::new(vec![
        skill("land", "open a pull request"),
        skill("gate", "cargo nextest"),
    ]);
    let (sender, records) = channel();
    let sender = Mutex::new(sender);
    let record: Record = Arc::new(move |record| {
        if let Ok(sender) = sender.lock() {
            let _ = sender.send(record);
        }
    });
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let deliver: Deliver = Arc::new(move |message| {
        if let (
            AgentMessage::Custom {
                content: UserContent::Text(text),
                ..
            },
            Ok(mut sink),
        ) = (message, sink.lock())
        {
            sink.push(text);
        }
    });
    let skills = vec![
        (
            "land".to_owned(),
            "commit, push, open and merge a forge PR".to_owned(),
        ),
        (
            "gate".to_owned(),
            "run the local gates and focused tests".to_owned(),
        ),
    ];
    let sidecar = Sidecar {
        url,
        key: None,
        model: "english".to_owned(),
        timeout: Duration::from_secs(2),
        threshold,
    };
    engine.set_classifier(Arc::new(SkillClassifier::new(
        sidecar, skills, record, deliver,
    )?));
    Ok(Rig {
        engine,
        records,
        delivered,
    })
}

fn typed(text: &str) -> AgentMessage {
    AgentMessage::user_input(UserContent::Text(text.to_owned()), 7)
}

async fn next(rig: &Rig, wait: Duration) -> Option<ClassifyRecord> {
    tokio::task::block_in_place(|| rig.records.recv_timeout(wait).ok())
}

fn delivered(rig: &Rig) -> Vec<String> {
    rig.delivered
        .lock()
        .map(|sink| sink.clone())
        .unwrap_or_default()
}

/// With no threshold the classifier only records: a paraphrase no trigger word matches is
/// journaled with its answer, and nothing points.
#[tokio::test(flavor = "multi_thread")]
async fn in_shadow_a_decision_is_recorded_and_nothing_fires() -> TestResult {
    let (port, served) = sidecar(vec![LAND])?;
    let rig = rig(format!("http://127.0.0.1:{port}"), None)?;
    assert!(
        rig.engine
            .observe_user(&typed("land this branch once it is green"))
            .is_empty()
    );
    let record = next(&rig, Duration::from_secs(5))
        .await
        .ok_or("no decision recorded")?;
    assert_eq!(
        (
            record.answer.as_deref(),
            record.confidence,
            record.model.as_deref(),
            record.fired
        ),
        (Some("land"), Some(0.91), Some("english"), false)
    );
    assert_eq!(
        record.message,
        yi_runtime::classifier::message_id("land this branch once it is green")
    );
    assert!(delivered(&rig).is_empty());
    let asked = served.join().map_err(|_| "sidecar thread")?;
    let body = asked.first().ok_or("one request")?;
    assert!(
        body.contains(r#""type":"choice""#) && body.contains(r#""none":"#),
        "{body}"
    );
    assert!(body.contains("land this branch"), "{body}");
    Ok(())
}

/// The join key `evals/skill_labels.py` computes for the same text; its test pins the same value.
#[test]
fn a_message_id_matches_the_labelling_tool() {
    assert_eq!(
        yi_runtime::classifier::message_id("  Land THIS branch\n\tonce the ÉTÉ gate is green "),
        "2990f761ef69"
    );
    assert_eq!(
        yi_runtime::classifier::message_id("a\u{b}b"),
        "98992d7f2eec"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn at_the_threshold_it_points_and_never_twice() -> TestResult {
    let (port, _served) = sidecar(vec![LAND, LAND])?;
    let rig = rig(format!("http://127.0.0.1:{port}"), Some(0.7))?;
    assert!(
        rig.engine
            .observe_user(&typed("land this branch once it is green"))
            .is_empty()
    );
    let record = next(&rig, Duration::from_secs(5))
        .await
        .ok_or("no decision recorded")?;
    assert!(record.fired);
    assert_eq!(
        delivered(&rig),
        ["Relevant: skill://land (the classifier, 0.91)"]
    );
    // A trigger word already pointed at land for this message: the classifier adds nothing.
    let pointed = rig.engine.observe_user(&typed("then open a pull request"));
    assert_eq!(pointed.len(), 1);
    let again = next(&rig, Duration::from_secs(5))
        .await
        .ok_or("no second decision")?;
    assert!(!again.fired, "{again:?}");
    assert_eq!(delivered(&rig).len(), 1);
    Ok(())
}

/// Three failures in a row pause the classifier and say so once; trigger words carry on.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_sidecar_trips_the_breaker_and_says_so_once() -> TestResult {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let rig = rig(format!("http://127.0.0.1:{port}"), Some(0.7))?;
    for turn in 0..3 {
        rig.engine.observe_user(&typed("land this branch"));
        let record = next(&rig, Duration::from_secs(5))
            .await
            .ok_or("no failure recorded")?;
        assert!(record.error.is_some(), "turn {turn}: {record:?}");
    }
    let notices = delivered(&rig);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices
            .iter()
            .all(|text| text.contains("failed 3 times") && text.contains("trigger words only")),
        "{notices:?}"
    );
    let pointed = rig.engine.observe_user(&typed("$gate please"));
    assert_eq!(
        pointed.len(),
        1,
        "a trigger word still points while the classifier is paused"
    );
    assert!(
        next(&rig, Duration::from_millis(500)).await.is_none(),
        "paused: nothing asked"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_the_user_did_not_type_is_never_classified() -> TestResult {
    let (port, _served) = sidecar(vec![LAND])?;
    let rig = rig(format!("http://127.0.0.1:{port}"), Some(0.7))?;
    let delegated = AgentMessage::host_user(UserContent::Text("land this branch".to_owned()), 7);
    assert!(rig.engine.observe_user(&delegated).is_empty());
    assert!(next(&rig, Duration::from_millis(500)).await.is_none());
    Ok(())
}
