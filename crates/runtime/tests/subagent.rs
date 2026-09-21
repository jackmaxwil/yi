//! The host's own rules for a child: how an exit reads, and what a spawn may draw (D215).

use yi_runtime::family::{MemberState, read_exit};
use yi_types::subagent::{ChildExit, ChildStatus, ChildUpdate, FailClass};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Dies with `read_exit` as the one reading: give any surface its own mapping and one of these
/// rows splits, which is the TUI showing `completed` for a child the model was told had failed.
#[test]
fn status_state_and_notice_derive_from_one_exit() -> TestResult {
    let provider = ChildExit::Failed {
        class: FailClass::Provider,
    };
    let rows = [
        (None, ChildStatus::Running, MemberState::Running, "running"),
        (
            Some(ChildExit::Completed),
            ChildStatus::Completed,
            MemberState::Finished,
            "finished",
        ),
        (
            Some(provider),
            ChildStatus::Error,
            MemberState::Failed,
            "failed",
        ),
        (
            Some(ChildExit::Interrupted),
            ChildStatus::Error,
            MemberState::Failed,
            "interrupted",
        ),
        (
            Some(ChildExit::Reaped),
            ChildStatus::Error,
            MemberState::Failed,
            "reaped",
        ),
        (
            Some(ChildExit::Repossessed),
            ChildStatus::Error,
            MemberState::Failed,
            "repossessed",
        ),
    ];
    for (exit, status, state, verb) in rows {
        let reading = read_exit(exit);
        assert_eq!(
            (reading.status, reading.state, reading.verb),
            (status, state, verb)
        );
    }
    // The wire keeps its three status words; the exit rides beside them and may be absent.
    let old = r#"{"id":"sub-1","name":"a","status":"error","activity":"waiting","toolUseCount":0,"tokenCount":0}"#;
    let parsed: ChildUpdate = serde_json::from_str(old)?;
    assert_eq!((parsed.status, parsed.exit), (ChildStatus::Error, None));
    assert_eq!(serde_json::to_string(&parsed)?, old);
    let typed = ChildUpdate {
        exit: Some(provider),
        ..parsed
    };
    assert!(
        serde_json::to_string(&typed)?.ends_with(r#""exit":{"kind":"failed","class":"provider"}}"#)
    );
    Ok(())
}
