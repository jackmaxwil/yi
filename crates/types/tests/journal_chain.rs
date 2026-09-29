//! A journal the plan store or the memory store wrote verifies line by line and re-serializes to
//! the bytes on disk; the fixtures came from the stores themselves.

use std::error::Error;
use std::path::PathBuf;

use yi_types::memory::MemoryRecord;
use yi_types::plan::canonical::Chained;
use yi_types::plan::ledger::JournalRecord;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture(name: &str) -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    Ok(std::fs::read_to_string(path)?)
}

#[test]
fn a_plan_journal_verifies_and_keeps_its_bytes() -> TestResult {
    let text = fixture("journals/plan-ops-v1.jsonl")?;
    let mut prev = None;
    for line in text.lines() {
        let record: JournalRecord = serde_json::from_str(line)?;
        assert_eq!(record.digest_of(prev.as_ref())?, record.digest, "{line}");
        assert_eq!(record.line()?, format!("{line}\n").into_bytes());
        prev = Some(record.digest);
    }
    assert!(prev.is_some(), "the fixture holds records");
    Ok(())
}

#[test]
fn a_memory_journal_verifies_and_keeps_its_bytes() -> TestResult {
    let text = fixture("journals/memory-ops-v1.jsonl")?;
    let mut prev = None;
    for line in text.lines() {
        let record: MemoryRecord = serde_json::from_str(line)?;
        assert_eq!(record.digest_of(prev.as_ref())?, record.digest, "{line}");
        assert_eq!(record.line()?, format!("{line}\n").into_bytes());
        prev = Some(record.digest);
    }
    assert!(prev.is_some(), "the fixture holds records");
    Ok(())
}
