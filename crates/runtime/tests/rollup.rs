//! The cross-session fold, over a root session and a child recorded by the real session writer
//! (scrubbed of text, in `fixtures/rollup`) laid out as `yi` lays them out on disk.

use std::error::Error;
use std::path::Path;

use yi_runtime::rollup::scan;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const ROOT: &str = include_str!("fixtures/rollup/root_glm_with_child_usage.jsonl");
const CHILD: &str = include_str!("fixtures/rollup/child_bedrock.jsonl");
const CHILD_REPLY: f64 = 0.010216;
/// The root fixture's two replies, summed by hand from its file: 0.0004671 + 0.000159415.
const ROOT_REPLIES: f64 = 0.000626515;

fn put(root: &Path, relative: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// A root, its child under `children/`, and a legacy `rlm-*/sub-*` child with its own reply id.
fn laid_out(name: &str) -> Result<Scratch, Box<dyn Error>> {
    let dir = Scratch::new(name)?;
    let stem = "--proj--/1787905015970_01a04771-80a2-727b-a35d-9a0bfe11023b";
    put(&dir, &format!("{stem}.jsonl"), ROOT)?;
    put(&dir, &format!("{stem}/children/sub-aa/c.jsonl"), CHILD)?;
    let legacy = CHILD
        .replace("R8jG4wschDCyIUWXZMx1", "legacyReplyId00000000")
        .replace("01a0faa8-5f44", "01a0faa8-aaaa");
    put(&dir, "rlm-2/sub-bb/c.jsonl", &legacy)?;
    Ok(dir)
}

/// Dies when a parent's `child_usage_attributed` records are folded in beside the children's
/// own files (the root fixture holds two worth $0.00022), or a legacy `rlm-*/sub-*` file is missed.
#[test]
fn each_reply_counts_once_across_root_child_and_legacy_files() -> TestResult {
    let dir = laid_out("yi-rollup-layout")?;
    let found = scan(&dir, 0);
    assert_eq!((found.unreadable, found.bad_lines), (0, 0));
    assert_eq!(found.requests.len(), 4);
    let billed: f64 = found.requests.iter().map(|r| r.billed()).sum();
    let want = ROOT_REPLIES + 2.0 * CHILD_REPLY;
    assert!((billed - want).abs() < 1e-9, "{billed} vs {want}");
    assert_eq!(found.requests.iter().filter(|r| r.child).count(), 2);
    let child = found.requests.iter().find(|r| r.child).ok_or("no child")?;
    assert_eq!(child.upstream.as_deref(), Some("Amazon Bedrock"));
    assert_eq!(child.model, "anthropic/claude-opus-5.5");
    Ok(())
}

/// Dies when a copy of a session under a second path doubles its dollars.
#[test]
fn a_reply_copied_into_a_second_file_is_counted_once() -> TestResult {
    let dir = laid_out("yi-rollup-dedupe")?;
    put(&dir, "pr-rounds/x/c.jsonl", CHILD)?;
    assert_eq!(scan(&dir, 0).requests.len(), 4);
    Ok(())
}

/// Dies when `--since` keeps a reply stamped before the window because its file was touched
/// after it: the root file's mtime is now, its replies are from August.
#[test]
fn a_window_keeps_replies_by_their_own_timestamp() -> TestResult {
    let dir = laid_out("yi-rollup-window")?;
    let found = scan(&dir, 1_790_000_000_000);
    assert_eq!(found.requests.len(), 2);
    assert!(found.requests.iter().all(|r| r.child));
    Ok(())
}

/// One bad part among good ones: an empty file with no header and a torn assistant line are
/// named, and the rest still count.
#[test]
fn an_unreadable_file_and_a_torn_line_are_counted_not_swallowed() -> TestResult {
    let dir = laid_out("yi-rollup-bad")?;
    put(&dir, "--proj--/empty.jsonl", "")?;
    let torn =
        format!("{CHILD}{{\"kind\":\"entry\",\"message\":{{\"role\":\"assistant\",\"model\":\n");
    put(
        &dir,
        "rlm-3/sub-cc/c.jsonl",
        &torn.replace("R8jG4wschDCyIUWXZMx1", "tornCopy"),
    )?;
    let found = scan(&dir, 0);
    assert_eq!((found.unreadable, found.bad_lines), (1, 1));
    assert_eq!(found.requests.len(), 5);
    Ok(())
}
