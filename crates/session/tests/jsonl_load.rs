use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use yi_session::{EntryQuery, LogOptions, load_session};

type TestResult = Result<(), Box<dyn Error>>;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> Result<TempDir, Box<dyn Error>> {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("yi-session-jsonl-{}-{unique}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(TempDir(dir))
}

fn golden_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../types/tests/fixtures/v4-golden.jsonl")
}

#[test]
fn loads_a_pi_generated_v4_session_file() -> TestResult {
    let dir = temp_dir()?;
    let path = dir.0.join("golden.jsonl");
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
    let dir = temp_dir()?;
    let path = dir.0.join("torn.jsonl");
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
    let dir = temp_dir()?;
    let path = dir.0.join("unterminated.jsonl");
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
    let dir = temp_dir()?;
    let path = dir.0.join("corrupt.jsonl");
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
