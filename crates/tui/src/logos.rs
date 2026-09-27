//! Provider and lab marks beside the picker's rows: kitty placements over the row's slot,
//! or a two-letter glyph where the terminal draws no images.

use std::sync::OnceLock;

use crate::app::{App, Bottom};
use crate::orb::kitty;

include!(concat!(env!("OUT_DIR"), "/logos.rs"));
const PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/logos.zz"));
pub const PX: usize = 32;
pub const COLS: u16 = 2;
/// Above the orb's pair, so the two owners never share an id.
pub const IMAGE_ID_BASE: u32 = 7610;

fn masks() -> &'static [u8] {
    static MASKS: OnceLock<Vec<u8>> = OnceLock::new();
    MASKS.get_or_init(|| miniz_oxide::inflate::decompress_to_vec_zlib(PACKED).unwrap_or_default())
}

/// An OpenRouter row is told apart by its lab; a native row by its provider.
pub fn key_for(provider: &str, id: &str) -> String {
    match provider {
        "openrouter" => id.split('/').next().unwrap_or(provider).to_owned(),
        other => other.to_owned(),
    }
}

pub fn index(key: &str) -> Option<usize> {
    KEYS.iter().position(|known| *known == key)
}

pub fn image_id(index: usize) -> u32 {
    IMAGE_ID_BASE.saturating_add(u32::try_from(index).unwrap_or(u32::MAX))
}

pub fn mask(index: usize) -> Option<&'static [u8]> {
    let size = PX.saturating_mul(PX);
    masks().get(index.saturating_mul(size)..index.saturating_add(1).saturating_mul(size))
}

pub fn rgba(mask: &[u8], ink: (u8, u8, u8)) -> Vec<u8> {
    mask.iter()
        .flat_map(|&alpha| [ink.0, ink.1, ink.2, alpha])
        .collect()
}

/// Two letters: the initials of a hyphenated name, else the first two.
pub fn glyph(key: &str) -> String {
    let mut parts = key.split('-').filter(|part| !part.is_empty());
    let first = parts.next().unwrap_or("?");
    let letters: String = match parts.next() {
        Some(second) => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
        None => first.chars().take(2).collect(),
    };
    format!("{:<2}", letters.to_uppercase())
}

#[derive(Default)]
pub struct Tick {
    placed: Vec<(u32, u16, u16)>,
}

fn wanted(app: &App) -> Vec<(u32, u16, u16)> {
    let (Some(Bottom::Model(popup)), Some((col, row0))) = (&app.bottom, app.logo_rows) else {
        return Vec::new();
    };
    popup
        .visible_keys()
        .iter()
        .enumerate()
        .filter_map(|(at, key)| {
            let row = row0.saturating_add(u16::try_from(at).unwrap_or(u16::MAX));
            index(key).map(|index| (image_id(index), col, row))
        })
        .collect()
}

/// Invariant: every placement here belongs to the open picker, so a change of rows deletes
/// them all and re-places what is wanted; a placement never outlives the popup that drew it.
/// The ids are fixed, so one holder at a time: the console releases an unfocused pane's.
pub fn tick(app: &App, out: &mut impl std::io::Write, state: &mut Tick) {
    if !app.kitty {
        return;
    }
    let wanted = wanted(app);
    if wanted == state.placed {
        return;
    }
    delete_all(out, state);
    for (placement, &(id, col, row)) in wanted.iter().enumerate() {
        let index = usize::try_from(id.saturating_sub(IMAGE_ID_BASE)).unwrap_or(usize::MAX);
        let Some(mask) = mask(index) else { continue };
        if !state.placed.iter().any(|placed| placed.0 == id) {
            let _ = kitty::transmit(out, id, &rgba(mask, kitty::INK_RGB), (PX, PX));
        }
        let placement = u32::try_from(placement.saturating_add(1)).unwrap_or(u32::MAX);
        if kitty::place_nth(out, id, placement, col, row, COLS, 1).is_ok() {
            state.placed.push((id, col, row));
        }
    }
}

pub fn delete_all(out: &mut impl std::io::Write, state: &mut Tick) {
    let mut ids: Vec<u32> = state.placed.iter().map(|placed| placed.0).collect();
    ids.dedup();
    for id in ids {
        let _ = kitty::delete_id(out, id);
    }
    state.placed.clear();
}
