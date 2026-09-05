//! The picker's logo masks (`data/logos/*.a8`, 32x32 coverage each) ship as one deflated blob
//! plus the key table in the same sorted order, so index and name never drift apart.
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    println!("cargo::rerun-if-changed=data/logos");
    let mut names: Vec<String> = std::fs::read_dir("data/logos")?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".a8").map(str::to_owned)
        })
        .collect();
    names.sort();
    let mut masks = Vec::new();
    for name in &names {
        masks.extend(std::fs::read(format!("data/logos/{name}.a8"))?);
    }
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&masks, 9);
    std::fs::File::create(out.join("logos.zz"))?.write_all(&packed)?;
    let quoted: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    let table = format!(
        "pub const KEYS: [&str; {}] = [{}];\n",
        names.len(),
        quoted.join(", ")
    );
    std::fs::File::create(out.join("logos.rs"))?.write_all(table.as_bytes())?;
    Ok(())
}
