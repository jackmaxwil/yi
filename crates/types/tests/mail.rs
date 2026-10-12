use yi_types::mail::{Delivery, Envelope, Kind, MailId, Receipt};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_envelope_round_trips_in_the_wire_spelling_the_inbox_stores() -> TestResult {
    let wire = r#"{"id":"main-17","from":"main","to":"tests","kind":"request","conversation":"main-17","seq":17,"sentAt":1757650000000,"deadlineMs":1757653600000,"body":"which suite is red?","ref":"family://findings-tests"}"#;
    let parsed: Envelope = serde_json::from_str(wire)?;
    assert_eq!(parsed.kind, Kind::Request);
    assert_eq!(parsed.in_reply_to, None);
    assert_eq!(serde_json::to_string(&parsed)?, wire);
    // An inbox written before services carries no incarnation, and still parses as it was.
    assert_eq!(
        (parsed.from_incarnation, parsed.to_incarnation),
        (None, None)
    );
    let addressed = wire.replace(r#""to":"tests","#, r#""to":"tests","toIncarnation":2,"#);
    let parsed: Envelope = serde_json::from_str(&addressed)?;
    assert_eq!(parsed.to_incarnation, Some(2));
    assert_eq!(serde_json::to_string(&parsed)?, addressed);
    Ok(())
}

#[test]
fn a_receipt_spells_its_state_in_lower_case() -> TestResult {
    let receipt = Receipt {
        target: "tests".to_owned(),
        id: MailId("main-17".to_owned()),
        state: Delivery::Inboxed,
        presented: String::new(),
    };
    assert_eq!(
        serde_json::to_string(&receipt)?,
        r#"{"target":"tests","id":"main-17","state":"inboxed"}"#
    );
    Ok(())
}

/// Dies with a letter between sessions losing its return address, provenance or hop count on
/// the way through a mail channel, or with family mail growing keys an old inbox never had.
#[test]
fn a_letter_between_sessions_round_trips_its_addresses_and_hops() -> TestResult {
    let wire = r#"{"id":"01a1-7f3e","from":"history://01a1","to":"history://01b2","kind":"request","conversation":"01a1-7f3e","seq":1791774661505,"sentAt":1791774661505,"body":"which runner image?","fromSession":"history://01a1","toSession":"history://01b2","origin":"history://01a1/e9","intent":["history://01a1/e4"],"hops":3}"#;
    let parsed: Envelope = serde_json::from_str(wire)?;
    assert_eq!(parsed.hops, yi_types::mail::Hops(3));
    assert_eq!(parsed.intent.len(), 1);
    assert_eq!(serde_json::to_string(&parsed)?, wire);
    let family = r#"{"id":"main-1","from":"main","to":"tests","kind":"inform","conversation":"main-1","seq":1,"sentAt":1,"body":"hi"}"#;
    let parsed: Envelope = serde_json::from_str(family)?;
    assert_eq!((parsed.hops.0, parsed.from_session.is_none()), (0, true));
    assert_eq!(serde_json::to_string(&parsed)?, family);
    assert_eq!(serde_json::to_string(&Delivery::Stored)?, r#""stored""#);
    Ok(())
}
