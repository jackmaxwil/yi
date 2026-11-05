//! The memory-injection eval replays the real injector over every labelled case and lets
//! `evals/memory_inject.py` score it against the committed pre-change block.

use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/fixtures/memory-inject")
}

fn copy(from: &Path, to: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy(&path, &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A store to read and an empty repository: the fixture's notes are all global.
fn staged() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let scratch = std::env::temp_dir().join("yi-memory-inject");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    let (home, cwd) = (scratch.join("home"), scratch.join("cwd"));
    copy(&fixture().join("home"), &home)?;
    std::fs::create_dir_all(&cwd)?;
    Ok((home, cwd))
}

/// The blocks this build renders for every labelled case, handed to the eval script to score.
#[test]
fn the_focused_block_scores_against_the_saved_labels() -> Result<(), Box<dyn std::error::Error>> {
    let labels: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture().join("labels.json"))?)?;
    let (home, cwd) = staged()?;
    let mut dump = String::new();
    for case in labels["cases"].as_array().into_iter().flatten() {
        let (Some(id), Some(prompt)) = (case["id"].as_str(), case["prompt"].as_str()) else {
            continue;
        };
        let (block, _) = yi_runtime::memory::focused(&home, &cwd, prompt);
        dump.push_str(&serde_json::json!({"id": id, "block": block}).to_string());
        dump.push('\n');
    }
    let path = std::env::temp_dir().join("yi-memory-inject-after.jsonl");
    std::fs::write(&path, dump)?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the contract under test is the eval script scoring blocks this build rendered"
    )]
    let out = std::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/memory_inject.py"))
        .arg("replay")
        .arg("--blocks")
        .arg(&path)
        .output()?;
    println!("{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}
