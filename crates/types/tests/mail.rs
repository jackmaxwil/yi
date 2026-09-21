use yi_types::mail::{Delivery, Envelope, Kind, MailId, Receipt};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_envelope_round_trips_in_the_wire_spelling_the_inbox_stores() -> TestResult {
    let wire = r#"{"id":"main-17","from":"main","to":"tests","kind":"request","conversation":"main-17","seq":17,"sentAt":1757650000000,"deadlineMs":1757653600000,"body":"which suite is red?","ref":"family://findings-tests"}"#;
    let parsed: Envelope = serde_json::from_str(wire)?;
    assert_eq!(parsed.kind, Kind::Request);
    assert_eq!(parsed.in_reply_to, None);
    assert_eq!(serde_json::to_string(&parsed)?, wire);
    Ok(())
}

#[test]
fn a_receipt_spells_its_state_in_lower_case() -> TestResult {
    let receipt = Receipt {
        target: "tests".to_owned(),
        id: MailId("main-17".to_owned()),
        state: Delivery::Inboxed,
    };
    assert_eq!(
        serde_json::to_string(&receipt)?,
        r#"{"target":"tests","id":"main-17","state":"inboxed"}"#
    );
    Ok(())
}
