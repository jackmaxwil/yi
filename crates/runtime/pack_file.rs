fn pack(
    source: &std::path::Path,
    out: &std::path::Path,
    stem: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read(source)?;
    std::str::from_utf8(&raw).map_err(|error| format!("{}: not UTF-8: {error}", source.display()))?;
    pack_bytes(&raw, out, stem)
}
