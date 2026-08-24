use std::path::PathBuf;

#[expect(
    clippy::disallowed_methods,
    reason = "the contract under test is a real python process importing the shipped package"
)]
#[test]
fn ready_check_accepts_the_shipped_yi_runtime_package() -> Result<(), Box<dyn std::error::Error>> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let src = repo.join("python").join("yi_runtime").join("src");
    let output = std::process::Command::new("python3")
        .args(["-c", yi_kernel::bootstrap::RUNTIME_READY_CHECK])
        .env("PYTHONPATH", &src)
        .current_dir(std::env::temp_dir())
        .output()?;
    assert!(
        output.status.success(),
        "RUNTIME_READY_CHECK must accept python/yi_runtime; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
