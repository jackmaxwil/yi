//! The picker's marks: which key a row gets, what the glyph reads, and that every shipped
//! mask is a full square with ink in it.

use yi_tui::logos::{KEYS, PX, glyph, image_id, index, key_for, mask, rgba};

#[test]
fn an_openrouter_row_is_keyed_by_its_lab_and_a_native_row_by_its_provider() {
    assert_eq!(key_for("openrouter", "openai/gpt-6-astra"), "openai");
    assert_eq!(key_for("openrouter", "qwen/qwen3.7-max"), "qwen");
    assert_eq!(key_for("anthropic", "claude-opus-5"), "anthropic");
    assert_eq!(key_for("faux", "faux-1"), "faux");
}

#[test]
fn the_glyph_is_two_letters_from_the_name() {
    assert_eq!(glyph("openai"), "OP");
    assert_eq!(glyph("z-ai"), "ZA");
    assert_eq!(glyph("bytedance-seed"), "BS");
    assert_eq!(glyph("google"), "GO");
    assert_eq!(glyph("x"), "X ");
}

/// A lab models.dev serves a placeholder for has no mask, so it takes the glyph.
#[test]
fn placeholder_labs_ship_no_mask() {
    for key in [
        "mistralai",
        "qwen",
        "z-ai",
        "bytedance-seed",
        "amazon",
        "faux",
    ] {
        assert!(index(key).is_none(), "{key}");
    }
}

#[test]
fn every_shipped_mask_is_a_full_square_with_ink() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        KEYS.contains(&"openai") && KEYS.contains(&"anthropic") && KEYS.contains(&"openrouter")
    );
    for (at, key) in KEYS.iter().enumerate() {
        let mask = mask(at).ok_or(format!("{key}: no mask"))?;
        assert_eq!(mask.len(), PX * PX, "{key}");
        let inked = mask.iter().filter(|&&alpha| alpha > 0).count();
        assert!(inked > PX * PX / 8, "{key}: {inked} inked pixels");
        assert_eq!(rgba(mask, (1, 2, 3)).len(), PX * PX * 4);
        assert_eq!(image_id(at), 7610 + u32::try_from(at)?);
    }
    assert!(mask(KEYS.len()).is_none());
    Ok(())
}
