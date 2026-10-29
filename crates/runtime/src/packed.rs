//! Incident: 54 KiB of raw prompt text cost the dist binary four 16 KiB `__TEXT` pages under the
//! 8 MiB cap, so `build.rs` deflates every prompt and the binary carries the deflate.
use std::sync::OnceLock;

/// One prompt the build deflated; [`Packed::text`] inflates it on first use and never again.
pub(crate) struct Packed {
    bytes: &'static [u8],
    text: OnceLock<String>,
}

impl Packed {
    pub(crate) const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            text: OnceLock::new(),
        }
    }

    /// Invariant: `build.rs` proves the round trip before the crate compiles, so a decode that
    /// failed here would name itself in the prompt rather than reach the model as silence.
    pub(crate) fn text(&self) -> &str {
        self.text.get_or_init(|| {
            miniz_oxide::inflate::decompress_to_vec_zlib(self.bytes)
                .map(|raw| String::from_utf8_lossy(&raw).into_owned())
                .unwrap_or_else(|error| format!("[prompt unavailable: {error:?}]"))
        })
    }
}
