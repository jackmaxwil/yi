include!("pack.rs");
include!("pack_file.rs");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=pack.rs");
    println!("cargo::rerun-if-changed=pack_file.rs");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    for dir in ["src/prompts", "src/goal/prompts"] {
        println!("cargo::rerun-if-changed={dir}");
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let prompt = path
                .extension()
                .is_some_and(|extension| extension == "md" || extension == "json");
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !prompt {
                continue;
            }
            pack(&path, &out, stem)?;
        }
    }
    Ok(())
}
