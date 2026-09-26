use yi_types::oauth::{CredentialFile, ProfileFile};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_v1_token_fixture_deserializes_and_keeps_unknown_fields() -> TestResult {
    let raw = include_str!("fixtures/oauth-token-v1.json");
    let token: CredentialFile = serde_json::from_str(raw)?;
    assert_eq!(token.version, 1);
    assert_eq!(token.kind, "oauth");
    assert_eq!(token.access, "fixture-access-token");
    assert_eq!(token.refresh.as_deref(), Some("fixture-refresh-token"));
    assert_eq!(token.expires_ms, 1_893_456_000_000);
    assert_eq!(token.account.as_deref(), Some("user@example.com"));
    assert_eq!(token.org.as_deref(), Some("org-fixture"));
    // A field this build does not know rides along in `extra` and survives a rewrite.
    assert_eq!(
        token.extra.get("device").and_then(|v| v.as_str()),
        Some("laptop")
    );
    let out = serde_json::to_string(&token)?;
    assert!(out.contains("\"device\":\"laptop\""), "{out}");
    Ok(())
}

#[test]
fn the_v1_profile_fixture_deserializes() -> TestResult {
    let raw = include_str!("fixtures/oauth-profile-v1.json");
    let profile: ProfileFile = serde_json::from_str(raw)?;
    assert_eq!(profile.version, 1);
    assert_eq!(profile.kind, "oauth-code");
    assert_eq!(profile.client_id.as_deref(), Some("fixture-client"));
    assert_eq!(profile.callback_port, Some(8317));
    Ok(())
}
