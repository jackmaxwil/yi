#[path = "support/scratch.rs"]
mod scratch;

use std::error::Error;

#[test]
fn scratch_dir_and_its_home_are_gone_after_drop() -> Result<(), Box<dyn Error>> {
    let dir = scratch::Scratch::new("yi-scratch-drop")?;
    let home = dir.home()?;
    std::fs::write(home.join("marker"), "left by a test")?;
    let path = dir.to_path_buf();
    drop(dir);
    assert!(!path.exists(), "{} survived its drop", path.display());
    Ok(())
}

#[test]
fn a_panicking_test_keeps_a_copy_of_its_scratch_dir() -> Result<(), Box<dyn Error>> {
    let mut path = std::path::PathBuf::new();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(dir) = scratch::Scratch::new("yi-scratch-kept") else {
            return;
        };
        let _ = std::fs::create_dir_all(dir.join("frames"));
        let _ = std::fs::write(dir.join("frames/0001.txt"), "the frame the assertion read");
        path = dir.to_path_buf();
        std::panic::resume_unwind(Box::new("the drive test's assertion"));
    }));
    assert!(failed.is_err() && !path.as_os_str().is_empty());
    let name = path.file_name().ok_or("scratch dir has no name")?;
    let kept = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("failed")
        .join(name);
    assert_eq!(
        std::fs::read_to_string(kept.join("frames/0001.txt"))?,
        "the frame the assertion read"
    );
    assert!(!path.exists(), "the temp dir itself still goes");
    std::fs::remove_dir_all(kept)?;
    Ok(())
}
