use yi_types::lease::{LeaseRecord, ParentClose};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_lease_record_round_trips_in_the_spelling_the_journal_stores() -> TestResult {
    let wire = r#"{"event":"repossessed","lease":{"holder":"tests","parent":"parent","deadlineMs":1757653600000,"tokens":4000,"grantedAt":1757650000000,"revoked":{"at":1757650100000,"graceMs":30000,"reason":"scope changed"}},"at":1757650130000,"kept":["history://tests","branch://yi/sub-1a2b3c4d"],"disposition":{"state":"settled"}}"#;
    let parsed: LeaseRecord = serde_json::from_str(wire)?;
    let LeaseRecord::Repossessed(record) = &parsed else {
        return Err("not a repossession".into());
    };
    assert_eq!(
        record.lease.revoked.as_ref().map(|revoked| revoked.due()),
        Some(1_757_650_130_000)
    );
    assert_eq!(serde_json::to_string(&parsed)?, wire);
    Ok(())
}

#[test]
fn parent_close_defaults_to_a_thirty_second_terminate_and_knows_no_abandon() -> TestResult {
    let terminate: ParentClose = serde_json::from_str(r#"{"policy":"terminate"}"#)?;
    assert_eq!(terminate, ParentClose::default());
    assert_eq!(terminate.grace_ms(), 30_000);
    assert!(serde_json::from_str::<ParentClose>(r#"{"policy":"abandon"}"#).is_err());
    Ok(())
}

/// Dies with `#[serde(other)]` on `ChildExit` and `FailClass`: an ending this build does not
/// know fails the whole `ChildUpdate`, and its status and error go with it.
#[test]
fn an_exit_from_a_newer_host_keeps_the_update_that_carries_it() -> TestResult {
    use yi_types::subagent::{ChildExit, ChildStatus, ChildUpdate, FailClass};
    let update = |exit: &str| {
        format!(
            r#"{{"id":"sub-1","name":"n","status":"error","activity":"waiting","toolUseCount":0,"tokenCount":0,"error":"gone","exit":{exit}}}"#
        )
    };
    let evicted: ChildUpdate = serde_json::from_str(&update(r#"{"kind":"evicted"}"#))?;
    assert_eq!(
        (evicted.status, evicted.error.as_deref(), evicted.exit),
        (ChildStatus::Error, Some("gone"), Some(ChildExit::Other))
    );
    let class: ChildUpdate = serde_json::from_str(&update(r#"{"kind":"failed","class":"quota"}"#))?;
    assert_eq!(
        class.exit,
        Some(ChildExit::Failed {
            class: FailClass::Other
        })
    );
    Ok(())
}
