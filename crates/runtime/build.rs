use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
            let raw = std::fs::read(&path)?;
            let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);
            let round = miniz_oxide::inflate::decompress_to_vec_zlib(&packed)
                .map_err(|error| format!("{}: {error:?}", path.display()))?;
            if round != raw {
                return Err(format!("{}: deflate round trip differs", path.display()).into());
            }
            std::fs::File::create(out.join(format!("{stem}.zz")))?.write_all(&packed)?;
        }
    }
    Ok(())
}
