//! The memory journal's append: it chains from the last record without replaying the rest.

use crate::scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use yi_runtime::journal::RECORD_CAP;
use yi_runtime::memory::Store;
use yi_types::memory::MemoryRecord;
use yi_types::plan::canonical::Chained;

type TestResult = Result<(), Box<dyn Error>>;

fn imported(label: &str) -> Result<(Scratch, Store), Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-memory-journal-{label}"))?;
    let store = Store::new(dir.join("store"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memory/claude");
    store.import(&fixture, None)?;
    Ok((dir, store))
}

fn journal(store: &Store) -> PathBuf {
    store.dir().join("ops.jsonl")
}

fn records(store: &Store) -> Result<Vec<MemoryRecord>, Box<dyn Error>> {
    let text = std::fs::read_to_string(journal(store))?;
    Ok(text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}

/// Extends the journal to `count` records with copies of its last real one, each sealed on
/// the one before it as the store's own append seals it.
fn grow(store: &Store, count: usize) -> TestResult {
    let mut text = std::fs::read_to_string(journal(store))?;
    let mut last: MemoryRecord = serde_json::from_str(text.lines().last().ok_or("empty")?)?;
    for _ in text.lines().count()..count {
        let prev = last.digest;
        last.at = last.at.saturating_add(1);
        last = last
            .seal(Some(&prev))
            .map_err(|error| format!("{error:?}"))?;
        let line = last.line().map_err(|error| format!("{error:?}"))?;
        text.push_str(std::str::from_utf8(&line)?);
    }
    std::fs::write(journal(store), text)?;
    Ok(())
}

fn fastest_read(store: &Store, query: &str) -> Result<Duration, Box<dyn Error>> {
    let note = store.resolve(query).ok_or("no such note")?;
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        store.mark_read(&note.name)?;
        best = best.min(started.elapsed());
    }
    Ok(best)
}

fn fifty_thousand_record_chain(
    store: &Store,
    before_large: impl FnOnce() -> TestResult,
) -> Result<Vec<MemoryRecord>, Box<dyn Error>> {
    fastest_read(store, "fgj-forge-cli")?;
    grow(store, 1_000)?;
    let small = fastest_read(store, "fgj-forge-cli")?;
    grow(store, 50_000)?;
    before_large()?;
    let large = fastest_read(store, "fgj-forge-cli")?;
    assert!(
        large <= small.saturating_mul(3),
        "a read at 50,000 records took {large:?}, at 1,000 {small:?}"
    );
    records(store)
}

#[test]
fn an_append_after_fifty_thousand_records_costs_what_the_first_one_does() -> TestResult {
    let (_dir, store) = imported("fifty-thousand")?;
    let chain = fifty_thousand_record_chain(&store, || Ok(()))?;
    assert_eq!(chain.len(), 50_005);
    for pair in chain.windows(2) {
        let [prev, next] = pair else { continue };
        assert_eq!(next.digest_of(Some(&prev.digest)).ok(), Some(next.digest));
    }
    Ok(())
}

#[test]
fn a_broken_chain_is_still_named_at_rebuild_and_the_next_append_chains_on() -> TestResult {
    let (_dir, store) = imported("broken-chain")?;
    let text = std::fs::read_to_string(journal(&store))?;
    let first = text.lines().next().ok_or("no first record")?;
    let edited = first.replacen("\"adopt\"", "\"edit!\"", 1);
    assert_ne!(edited, first, "the edit changed the record in place");
    std::fs::write(journal(&store), text.replacen(first, &edited, 1))?;
    let note = store.resolve("fgj-forge-cli").ok_or("no such note")?;
    store.mark_read(&note.name)?;
    let chain = records(&store)?;
    let [.., before, read] = chain.as_slice() else {
        return Err(format!("{} records", chain.len()).into());
    };
    assert_eq!(read.op, "read");
    assert_eq!(read.digest_of(Some(&before.digest)).ok(), Some(read.digest));
    assert_eq!(store.rebuild()?.broken, Some(1));
    Ok(())
}

fn over_cap_and_valid(store: &Store) -> Result<(Vec<u8>, MemoryRecord), Box<dyn Error>> {
    let valid = records(store)?
        .into_iter()
        .next()
        .ok_or("no imported record")?;
    let mut oversized = valid.clone();
    oversized
        .extra
        .insert("padding".to_owned(), Value::String(String::new()));
    oversized = oversized.seal(None).map_err(|error| format!("{error:?}"))?;
    let base = oversized.line().map_err(|error| format!("{error:?}"))?;
    let target = RECORD_CAP.saturating_add(1);
    let padding = target
        .checked_sub(base.len())
        .ok_or("record already over cap")?;
    oversized
        .extra
        .insert("padding".to_owned(), Value::String("x".repeat(padding)));
    oversized = oversized.seal(None).map_err(|error| format!("{error:?}"))?;
    let bytes = oversized.line().map_err(|error| format!("{error:?}"))?;
    assert_eq!(bytes.len(), target);
    Ok((bytes, valid))
}

#[test]
fn a_valid_record_after_an_over_cap_line_parents_the_next_append() -> TestResult {
    let (_dir, store) = imported("over-cap-then-valid")?;
    let (mut damage, valid) = over_cap_and_valid(&store)?;
    let valid_line = valid.line().map_err(|error| format!("{error:?}"))?;
    let parent = valid.digest;
    let chain = fifty_thousand_record_chain(&store, || {
        let mut bytes = std::fs::read(journal(&store))?;
        bytes.append(&mut damage);
        bytes.extend_from_slice(&valid_line);
        std::fs::write(journal(&store), bytes)?;
        Ok(())
    })?;
    let [.., read, _, _, _, _] = chain.as_slice() else {
        return Err("fewer than five appends followed the valid record".into());
    };
    assert_eq!(read.digest_of(Some(&parent)).ok(), Some(read.digest));
    assert_eq!(read.op, "read");
    Ok(())
}
