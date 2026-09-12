use std::error::Error;
use std::fs;
use std::path::PathBuf;

use yi_session::{EntryQuery, JsonlRepo, LogOptions, SessionRepo, load_session};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn golden_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../types/tests/fixtures/v4-golden.jsonl")
}

#[test]
fn loads_a_pi_generated_v4_session_file() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("golden.jsonl");
    fs::copy(golden_fixture(), &path)?;
    let store = load_session(&path)?;

    assert_eq!(store.metadata().id, "fixture-a");
    let entries = store.find_entries(&EntryQuery::default())?;
    assert!(!entries.is_empty());
    let log = store.log(&LogOptions::default())?;
    assert_eq!(log.len(), 40);
    assert!(store.lanes().iter().any(|pointer| pointer.lane == "thread"));
    assert!(store.stats().message_count > 0);
    Ok(())
}

#[test]
fn repairs_a_torn_tail_by_dropping_the_partial_line() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("torn.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"torn","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\n{entry}\n{{\"kind\":\"ent"))?;

    let store = load_session(&path)?;
    assert_eq!(store.find_entries(&EntryQuery::default())?.len(), 1);
    let repaired = fs::read_to_string(&path)?;
    assert_eq!(repaired, format!("{header}\n{entry}\n"));
    Ok(())
}

#[test]
fn reterminates_a_file_missing_its_trailing_newline() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("unterminated.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"unterminated","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\n{entry}"))?;

    let store = load_session(&path)?;
    assert_eq!(store.find_entries(&EntryQuery::default())?.len(), 1);
    let repaired = fs::read_to_string(&path)?;
    assert!(repaired.ends_with('\n'));
    Ok(())
}

#[test]
fn rejects_a_mid_file_corrupt_line() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("corrupt.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"corrupt","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\nnot json\n{entry}\n"))?;

    match load_session(&path) {
        Ok(_) => Err("expected invalid_entry for mid-file corruption".into()),
        Err(error) => {
            assert_eq!(error.code(), "invalid_entry");
            Ok(())
        }
    }
}

/// A listing names a session from its first prompt, and a name fact written later wins.
#[test]
fn list_names_sessions_from_the_first_prompt_or_the_name_fact() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let mut repo = JsonlRepo::new(dir.to_path_buf(), "/tmp/yi-named");
    let header = |id: &str, at: u64| {
        format!(
            r#"{{"kind":"header","version":4,"id":"{id}","createdAt":{at},"cwd":"/tmp/yi-named"}}"#
        )
    };
    let user = r#"{"kind":"entry","lane":"main","type":"message","id":"e1","message":{"role":"user","content":"  fix the login bug\nand the logout one","timestamp":0},"parentId":null,"seq":1,"timestamp":1}"#;
    let fact = r#"{"kind":"fact","seq":2,"fact":"name","name":"login work"}"#;
    let sessions = dir.join("--tmp-yi-named--");
    fs::create_dir_all(&sessions)?;
    fs::write(
        sessions.join("1_first.jsonl"),
        format!("{}\n{user}\n", header("first", 1)),
    )?;
    fs::write(
        sessions.join("2_named.jsonl"),
        format!("{}\n{user}\n{fact}\n", header("named", 2)),
    )?;
    fs::write(
        sessions.join("3_bare.jsonl"),
        format!("{}\n", header("bare", 3)),
    )?;
    let listed = repo.list()?;
    let names: Vec<(String, Option<String>)> = listed
        .iter()
        .map(|metadata| (metadata.id.clone(), metadata.name.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("bare".to_owned(), None),
            ("named".to_owned(), Some("login work".to_owned())),
            ("first".to_owned(), Some("fix the login bug".to_owned())),
        ]
    );
    Ok(())
}
