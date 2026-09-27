//! `yi --version` is the `version:` line of docs/ARCHITECTURE.md; the workspace's `0.2.0` never
//! moved, so every measured build reported the same version. No line fails the build.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let map = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR unset")?,
    )
    .join("../../docs/ARCHITECTURE.md");
    println!("cargo::rerun-if-changed={}", map.display());
    let text = std::fs::read_to_string(&map)?;
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("version:"))
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or("docs/ARCHITECTURE.md has no `version:` line")?;
    println!("cargo::rustc-env=ARCHITECTURE_VERSION={version}");
    // A release-derived build ships stripped, so the function-starts table only told a profiler
    // where functions begin in a binary without symbols: 45 KB of it (D274).
    let macos = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos");
    if macos && std::env::var("PROFILE").is_ok_and(|profile| profile == "release") {
        println!("cargo::rustc-link-arg-bins=-Wl,-no_function_starts");
    }
    Ok(())
}
