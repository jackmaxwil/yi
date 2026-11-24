//! The screen surface: [`Screen::push`] and [`Screen::push_png`] from any thread, [`surface`] draws
//! it and hands input to a callback, [`marks`] draws labelled boxes. It knows nothing of the page.

use std::io::Cursor;

use png::{ColorType, Decoder, Limits, Transformations};
use rune::{Align, Drag, El, KeyEvent, Phase, Pixels, SurfaceFit, Tx, WheelEvent, col, el, text};
use yd_core::tokens::Tokens;

use crate::pane::Scene;

use crate::screen::input::{Input, Point, named, to_picture};
use crate::theme::look::colour;
use crate::theme::scale::ON_MODEL;

/// A mark's label: 10 at 600, as the record's agent tag.
const TAG: rune::TextStyle = rune::TextStyle::new(10.0, 600.0).lh(14.0);

pub const SCENES: &[Scene] = &[];

/// The longest side a screen takes.
pub const MAX_SIDE: u32 = 8_192;
/// The most pixels a decoded frame may hold, an 8K page: what a page can make the app
/// allocate for one frame is four bytes of each.
pub const MAX_PIXELS: u64 = 8_192 * 4_320;

/// What the decoder may hold beside the picture: a colour profile and other small chunks.
const BESIDE: usize = 1 << 20;

/// A picture refused before it reaches the drawing, which would panic on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadPicture {
    /// No pixels, more than [`MAX_SIDE`] a side, or a frame of more than [`MAX_PIXELS`].
    Size {
        width: u32,
        height: u32,
    },
    Length {
        wanted: usize,
        got: usize,
    },
    /// Not a PNG, or one cut short or damaged: what the decoder said.
    Damaged(String),
}

impl std::fmt::Display for BadPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BadPicture::Size { width, height } => write!(
                f,
                "a {width} by {height} picture is empty, passes {MAX_SIDE} a side or holds \
                 more than {MAX_PIXELS} pixels"
            ),
            BadPicture::Length { wanted, got } => {
                write!(f, "a picture of {wanted} bytes came with {got}")
            }
            BadPicture::Damaged(why) => write!(f, "the PNG did not read: {why}"),
        }
    }
}

impl std::error::Error for BadPicture {}

/// The latest picture of one screen. Clones share it: the source's thread holds one.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    pixels: Pixels,
}

impl Screen {
    /// Replaces the picture: `rgba` is straight-alpha sRGB, rows top first. Wakes only the
    /// windows drawing it and never runs a view.
    pub fn push(&self, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), BadPicture> {
        let sides = 1..=MAX_SIDE;
        if !sides.contains(&width) || !sides.contains(&height) {
            return Err(BadPicture::Size { width, height });
        }
        let wanted = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(BadPicture::Size { width, height })?;
        if rgba.len() != wanted {
            let got = rgba.len();
            return Err(BadPicture::Length { wanted, got });
        }
        self.pixels.set(width, height, rgba);
        Ok(())
    }

    /// Decodes a PNG on the caller's thread, then pushes it as [`Screen::push`] does.
    pub fn push_png(&self, png: &[u8]) -> Result<(), BadPicture> {
        let frame = decode(png)?;
        self.push(frame.width, frame.height, frame.rgba)
    }

    /// The picture's size in pixels; none before the first.
    pub fn size(&self) -> Option<(u32, u32)> {
        let (w, h) = self.pixels.size();
        (w > 0 && h > 0).then_some((w, h))
    }
}

/// A frame decoded: straight-alpha RGBA, rows top first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes one PNG frame. The header is read and checked against [`MAX_SIDE`] and
/// [`MAX_PIXELS`] first, so a frame that claims more is refused before it is allocated for.
pub fn decode(bytes: &[u8]) -> Result<Frame, BadPicture> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::normalize_to_color8());
    let damaged = |why: png::DecodingError| BadPicture::Damaged(why.to_string());
    let (width, height) = decoder.read_header_info().map_err(damaged)?.size();
    let sides = 1..=MAX_SIDE;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if !sides.contains(&width) || !sides.contains(&height) || pixels > MAX_PIXELS {
        return Err(BadPicture::Size { width, height });
    }
    let room = usize::try_from(pixels.saturating_mul(4)).unwrap_or(usize::MAX);
    decoder.set_limits(Limits {
        bytes: room.saturating_add(BESIDE),
    });
    let mut reader = decoder.read_info().map_err(damaged)?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= room)
        .ok_or(BadPicture::Size { width, height })?;
    let mut drawn = vec![0; size];
    let info = reader.next_frame(&mut drawn).map_err(damaged)?;
    let drawn = drawn.get(..info.buffer_size()).unwrap_or(&drawn);
    let rgba = match info.color_type {
        ColorType::Rgba => drawn.to_vec(),
        ColorType::Rgb => spread(drawn, |&[r, g, b]: &[u8; 3]| [r, g, b, 255]),
        ColorType::Grayscale => spread(drawn, |&[v]: &[u8; 1]| [v, v, v, 255]),
        ColorType::GrayscaleAlpha => spread(drawn, |&[v, a]: &[u8; 2]| [v, v, v, a]),
        ColorType::Indexed => return Err(BadPicture::Damaged("a palette was left".to_owned())),
    };
    Ok(Frame {
        width,
        height,
        rgba,
    })
}

/// Every `N` bytes of `drawn` as one RGBA pixel.
fn spread<const N: usize>(drawn: &[u8], pixel: impl Fn(&[u8; N]) -> [u8; 4]) -> Vec<u8> {
    drawn.as_chunks::<N>().0.iter().flat_map(pixel).collect()
}

/// A box to draw over the picture, in its pixels, with a label under its bottom-left corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub at: Point,
    pub width: u32,
    pub height: u32,
    pub label: String,
}

/// The picture fitted whole to the box it is given, held top-left, keyed `key`. Presses, drags,
/// wheels and keys on it, and the box's size, reach `on` as [`Input`].
pub fn surface<S: 'static>(
    key: &str,
    screen: &Screen,
    on: impl Fn(&mut S, Input) + Clone + 'static,
) -> El {
    let (drag, wheel, keys, laid) = (on.clone(), on.clone(), on.clone(), on);
    let shown = screen.clone();
    let focus = key.to_owned();
    el().key(key)
        .w_full()
        .h_full()
        .min_h(0.0)
        .clip_rounded(0.0)
        .pixels_fit(&screen.pixels, SurfaceFit::Fit)
        .surface_anchor(Align::Start, Align::Start)
        .focusable(true)
        .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if d.phase == Phase::Down {
                tx.focus(focus.clone(), false);
            }
            let Some(size) = shown.size() else { return };
            let Some(at) = to_picture(size, (d.w, d.h), (d.x, d.y)) else {
                return;
            };
            drag(
                s,
                match d.phase {
                    Phase::Down => Input::Press(at),
                    Phase::Move => Input::Move(at),
                    Phase::Up => Input::Release(at),
                },
            );
        })
        .on_layout_tx(move |s: &mut S, r: [f32; 4], _: &mut Tx| {
            let side = |v: f32| v.max(0.0).round() as u32;
            let (width, height) = (side(r[2]), side(r[3]));
            laid(s, Input::Resized { width, height });
        })
        .on_wheel(move |s: &mut S, e: WheelEvent, _: &mut Tx| {
            let (dx, dy) = (e.dx.round() as i32, e.dy.round() as i32);
            wheel(s, Input::Wheel { dx, dy });
            true
        })
        .on_key(move |s: &mut S, ev: &KeyEvent, _: &mut Tx| {
            let input = match &ev.key {
                rune::Key::Text(typed) if !ev.mods.cmd => Input::Text(typed.clone()),
                other => match named(other) {
                    Some(named) => Input::Key(named),
                    None => return false,
                },
            };
            keys(s, input);
            true
        })
}

/// The marks over a picture of `screen`, as a layer the size of the surface that takes no
/// input. Empty before the first picture: a mark has nowhere to be.
pub fn marks(tokens: &Tokens, screen: &Screen, marks: &[Mark]) -> El {
    let layer = col().w_full().h_full();
    let Some((pw, ph)) = screen.size().filter(|_| !marks.is_empty()) else {
        return layer;
    };
    // Invariant: grow shares the picture's side between empty spacers, so a mark is placed in
    // the picture's own pixels whatever the box it is fitted to.
    let share = |parts: [u32; 3], of: u32, across: bool| {
        let first = parts[0].min(of);
        let mid = parts[1].min(of.saturating_sub(first));
        let rest = of.saturating_sub(first).saturating_sub(mid);
        let gap = |v: u32| el().grow(v as f32).min_w(0.0).min_h(0.0);
        let line = if across { rune::row() } else { col() };
        (line.w_full().h_full(), gap(first), mid as f32, gap(rest))
    };
    let boxes = marks.iter().map(|mark| {
        let tag = text(mark.label.as_str())
            .font(TAG)
            .color(colour(ON_MODEL.of(tokens)))
            .fill(colour(tokens.model))
            .px(6.0)
            .radius(9.0)
            .pin(Some(0.0), None, None, Some(-17.0));
        let (row, left, wide, right) = share([mark.at.x, mark.width, 0], pw, true);
        let framed = col()
            .grow(wide)
            .h_full()
            .min_w(0.0)
            .border(1.0, colour(tokens.model))
            .radius(2.0)
            .child(tag);
        let (column, top, tall, bottom) = share([mark.at.y, mark.height, 0], ph, false);
        let across = row.grow(tall).h(0.0).child(left).child(framed).child(right);
        column
            .absolute(0.0, 0.0)
            .child(top)
            .child(across)
            .child(bottom)
    });
    layer.child(el().aspect_fit(pw as f32 / ph as f32).children(boxes))
}

#[cfg(test)]
mod tests;
