use std::error::Error;
use std::fs;
use std::path::Path;

use yi_types::wire::{JsonlV4Header, Mutation};

fn roundtrip_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let mut rebuilt = String::new();
    for (index, line) in content.lines().enumerate() {
        let reencoded = if index == 0 {
            let header: JsonlV4Header = serde_json::from_str(line)?;
            serde_json::to_string(&header)?
        } else {
            let mutation: Mutation = serde_json::from_str(line)?;
            serde_json::to_string(&mutation)?
        };
        assert_eq!(
            reencoded,
            line,
            "byte drift in {} line {}",
            path.display(),
            index + 1
        );
        rebuilt.push_str(&reencoded);
        rebuilt.push('\n');
    }
    assert_eq!(rebuilt, content, "whole-file drift in {}", path.display());
    Ok(())
}

#[test]
fn pi_v4_fixtures_roundtrip_byte_identical() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut seen = 0;
    for fixture in fs::read_dir(&dir)? {
        let path = fixture?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            roundtrip_file(&path)?;
            seen += 1;
        }
    }
    assert!(seen >= 2, "expected at least 2 jsonl fixtures, saw {seen}");
    Ok(())
}
