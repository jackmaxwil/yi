/// Writes `raw` deflated to `{stem}.zz` under `out`, refusing a deflate that does not inflate
/// back to the same bytes; the runtime, ai, kernel and tui build scripts share it by `include!`.
fn pack_bytes(
    raw: &[u8],
    out: &std::path::Path,
    stem: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(raw, 9);
    let round = miniz_oxide::inflate::decompress_to_vec_zlib(&packed)
        .map_err(|error| format!("{stem}: {error:?}"))?;
    if round != raw {
        return Err(format!("{stem}: deflate round trip differs").into());
    }
    std::fs::write(out.join(format!("{stem}.zz")), packed)?;
    Ok(())
}
