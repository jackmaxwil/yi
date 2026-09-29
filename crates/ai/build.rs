//! The five model catalogs are 146,182 bytes of JSON the binary would carry raw; deflated
//! here they are 11,857. `miniz_oxide` is already in the graph, via `yi-orb`.
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    for name in [
        "anthropic",
        "openai",
        "openrouter",
        "openai-codex",
        "google",
    ] {
        let source = format!("data/{name}.json");
        println!("cargo::rerun-if-changed={source}");
        let raw = std::fs::read(&source)?;
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);
        std::fs::File::create(out.join(format!("{name}.zz")))?.write_all(&packed)?;
    }
    Ok(())
}
