//! A journal the plan store or the memory store wrote verifies line by line and re-serializes to
//! the bytes on disk; the fixtures came from the stores themselves.

use std::error::Error;
use std::path::PathBuf;

use yi_types::memory::MemoryRecord;
use yi_types::plan::canonical::{Chained, Digest};
use yi_types::plan::ledger::JournalRecord as J;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture(name: &str) -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/journals")
        .join(format!("{name}-v1.jsonl"));
    Ok(std::fs::read_to_string(path)?)
}

fn check<T>(name: &str, digest: impl Fn(&T) -> Digest) -> TestResult
where
    T: Chained + serde::de::DeserializeOwned,
{
    let text = fixture(name)?;
    let mut prev = None;
    for line in text.lines() {
        let record: T = serde_json::from_str(line)?;
        let recorded = digest(&record);
        assert_eq!(record.digest_of(prev.as_ref())?, recorded, "{line}");
        assert_eq!(record.line()?, format!("{line}\n").into_bytes());
        prev = Some(recorded);
    }
    assert!(prev.is_some(), "{name} holds records");
    Ok(())
}

#[test]
fn a_plan_journal_verifies_and_keeps_its_bytes() -> TestResult {
    check::<J>("plan-ops", |r| r.digest)?;
    check::<J>("plan-judge-verdict", |r| r.digest)?;
    Ok(())
}

#[test]
fn a_memory_journal_verifies_and_keeps_its_bytes() -> TestResult {
    check::<MemoryRecord>("memory-ops", |r| r.digest)
}
