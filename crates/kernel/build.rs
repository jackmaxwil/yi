//! `python/yi_runtime` and `python/skills` ride in the binary as one deflated stream
//! (`u32 LE` path length, path, `u32 LE` content length, content, repeated), so a `yi`
//! installed away from its checkout still has a runtime to unpack under `~/.yi/python`. The
//! test suites stay out: they run from the checkout, never from the unpacked copy.
use std::io::Write;
use std::path::{Path, PathBuf};

type BuildResult<T> = Result<T, Box<dyn std::error::Error>>;

fn skipped(name: &str) -> bool {
    name.starts_with('.') || name == "__pycache__" || name == "tests" || name.ends_with(".pyc")
}

fn push(out: &mut Vec<u8>, bytes: &[u8]) -> BuildResult<()> {
    out.extend_from_slice(&u32::try_from(bytes.len())?.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<u8>) -> BuildResult<()> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if skipped(&name) {
            continue;
        }
        if path.is_dir() {
            walk(&path, root, out)?;
            continue;
        }
        let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
        push(out, relative.as_bytes())?;
        push(out, &std::fs::read(&path)?)?;
    }
    Ok(())
}

fn main() -> BuildResult<()> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR unset")?)
            .join("..")
            .join("..")
            .join("python");
    let mut stream = Vec::new();
    for tree in ["yi_runtime", "skills"] {
        let dir = root.join(tree);
        println!("cargo::rerun-if-changed={}", dir.display());
        walk(&dir, &root, &mut stream)?;
    }
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&stream, 9);
    std::fs::File::create(out.join("python.zz"))?.write_all(&packed)?;
    Ok(())
}
