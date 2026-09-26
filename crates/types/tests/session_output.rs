use std::error::Error;
use std::fs;
use std::path::Path;

use yi_types::wire::{SessionMetadata, SessionStats};

// Both fixtures are stdout captured from the 0.309.0 `yi` binary over sessions seeded
// from v4-golden.jsonl and v4-child-header.jsonl: `sessions --json list` and rpc frames.
fn fixture(name: &str) -> Result<String, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    Ok(fs::read_to_string(path)?.trim_end().to_owned())
}

#[test]
fn sessions_list_json_keeps_its_bytes() -> Result<(), Box<dyn Error>> {
    let listed = [
        SessionMetadata {
            id: "fixture-b".to_owned(),
            created_at: 1_787_544_431_475,
            parent_session_id: Some("fixture-a".to_owned()),
            name: None,
        },
        SessionMetadata {
            id: "fixture-a".to_owned(),
            created_at: 1_787_544_431_469,
            parent_session_id: None,
            name: Some("Golden Fixture v4".to_owned()),
        },
    ];
    assert_eq!(
        serde_json::to_string(&listed)?,
        fixture("sessions-list-v1.json")?
    );
    Ok(())
}

#[test]
fn get_session_stats_data_keeps_its_bytes() -> Result<(), Box<dyn Error>> {
    let frames = [
        SessionStats::zero(),
        SessionStats {
            message_count: 9,
            cached_tokens: 9000,
            uncached_tokens: 1430,
            total_tokens: 10774,
            cost_total: 0.007432,
        },
    ];
    assert_eq!(
        serde_json::to_string(&frames)?,
        fixture("get-session-stats-v1.json")?
    );
    Ok(())
}
