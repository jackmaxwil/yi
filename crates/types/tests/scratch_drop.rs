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
