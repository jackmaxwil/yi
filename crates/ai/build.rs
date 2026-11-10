//! The model catalogs are JSON the binary would carry raw; deflated here they are smaller.
//! `miniz_oxide` is already in the graph, via `yi-orb`.
include!("../runtime/pack.rs");
include!("../runtime/pack_file.rs");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=../runtime/pack.rs");
    println!("cargo::rerun-if-changed=../runtime/pack_file.rs");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    for name in [
        "anthropic",
        "openai",
        "openrelay",
        "openrouter",
        "openai-codex",
        "google",
        "cursor",
        "kimi",
    ] {
        let source = format!("data/{name}.json");
        println!("cargo::rerun-if-changed={source}");
        pack(std::path::Path::new(&source), &out, name)?;
    }
    Ok(())
}
