/// Writes `raw` deflated to `{stem}.zz` under `out`, refusing a deflate that does not inflate
/// back to the same bytes; the runtime, kernel and tui build scripts share it by `include!`.
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

/// [`pack_bytes`] for a text file, refused unless it is UTF-8, which `include_str!` once enforced.
#[allow(dead_code, reason = "the kernel and tui scripts pack bytes they assemble, never a file")]
fn pack(
    source: &std::path::Path,
    out: &std::path::Path,
    stem: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read(source)?;
    std::str::from_utf8(&raw).map_err(|error| format!("{}: not UTF-8: {error}", source.display()))?;
    pack_bytes(&raw, out, stem)
}
