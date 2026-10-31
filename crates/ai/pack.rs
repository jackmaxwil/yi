/// Writes `source` deflated to `{stem}.zz` under `out`, refusing a deflate that does not inflate
/// back to the same bytes; `crates/ai` and `crates/runtime` build scripts share it by `include!`.
fn pack(
    source: &std::path::Path,
    out: &std::path::Path,
    stem: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read(source)?;
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);
    let round = miniz_oxide::inflate::decompress_to_vec_zlib(&packed)
        .map_err(|error| format!("{}: {error:?}", source.display()))?;
    if round != raw {
        return Err(format!("{}: deflate round trip differs", source.display()).into());
    }
    std::fs::write(out.join(format!("{stem}.zz")), packed)?;
    Ok(())
}
