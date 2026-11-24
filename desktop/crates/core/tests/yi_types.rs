use std::error::Error;

use yi_types::entry::Entry;

// One line of Yi's v4 golden session fixture, written by Yi's own session store.
const THINKING_LEVEL: &str = r#"{"kind":"entry","lane":"main","type":"thinking_level_change","id":"e12","thinkingLevel":"high","parentId":"e11","seq":12,"timestamp":1787544431473}"#;

#[test]
fn a_session_entry_from_yi_reads_as_its_wire_type() -> Result<(), Box<dyn Error>> {
    let entry: Entry = serde_json::from_str(THINKING_LEVEL)?;
    let Entry::ThinkingLevelChange {
        thinking_level,
        parent_id,
        seq,
        ..
    } = entry
    else {
        return Err(format!("read as another entry kind: {entry:?}").into());
    };
    assert_eq!(
        (thinking_level.as_str(), parent_id.as_deref(), seq),
        ("high", Some("e11"), 12)
    );
    Ok(())
}
