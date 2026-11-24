use std::cell::Cell;
use std::error::Error;

use rune::{Cx, El, Env, Runtime, col};

use super::{BadPicture, MAX_PIXELS, MAX_SIDE, Screen, decode, surface};
use crate::screen::input::{Input, Point};

struct State {
    screen: Screen,
    views: Cell<u32>,
    seen: Vec<Input>,
}

fn view(state: &State, _cx: &mut Cx<State>) -> El {
    state.views.set(state.views.get().saturating_add(1));
    let on = |s: &mut State, input: Input| s.seen.push(input);
    col()
        .w_full()
        .h_full()
        .pl(40.0)
        .pt(30.0)
        .child(surface("screen", &state.screen, on))
}

fn runtime(size: (f32, f32)) -> Runtime<State> {
    let state = State {
        screen: Screen::default(),
        views: Cell::new(0),
        seen: Vec::new(),
    };
    Runtime::new(state, view, size, Env::new(true, 1.0))
}

#[test]
fn a_frame_pushed_from_a_thread_reaches_the_surface_without_a_view_run()
-> Result<(), Box<dyn Error>> {
    let mut rt = runtime((440.0, 330.0));
    // The first layout reports the box's size, which runs the view once more.
    rt.frame(0.016);
    rt.frame(0.016);
    let views = rt.state.views.get();
    let screen = rt.state.screen.clone();
    let pushed = std::thread::spawn(move || screen.push(4, 2, vec![255; 32]));
    pushed.join().map_err(|_| "the pushing thread died")??;
    rt.frame(0.016);
    assert_eq!(rt.state.screen.size(), Some((4, 2)));
    assert_eq!(
        rt.state.views.get(),
        views,
        "a picture must not run the view"
    );
    Ok(())
}

#[test]
fn a_picture_of_the_wrong_length_is_refused_not_drawn() -> Result<(), Box<dyn Error>> {
    let screen = Screen::default();
    let refused = screen.push(4, 2, vec![0; 31]);
    assert_eq!(
        refused,
        Err(BadPicture::Length {
            wanted: 32,
            got: 31
        })
    );
    assert_eq!(
        screen.push(0, 2, Vec::new()),
        Err(BadPicture::Size {
            width: 0,
            height: 2
        })
    );
    assert_eq!(screen.size(), None);
    Ok(())
}

/// The box is the window less the 40 by 30 offset; a 200 by 100 picture is fitted inside it.
#[test]
fn a_press_inside_the_fitted_picture_maps_to_its_page_point() -> Result<(), Box<dyn Error>> {
    for (window, scale) in [((440.0, 330.0), 2.0), ((240.0, 230.0), 1.0)] {
        let mut rt = runtime(window);
        rt.state.screen.push(200, 100, vec![255; 200 * 100 * 4])?;
        rt.frame(0.016);
        rt.state.seen.clear();
        let (x, y) = (40.0 + 150.25 * scale, 30.0 + 60.25 * scale);
        rt.pointer_down(x, y);
        rt.frame(0.016);
        rt.pointer_up(x, y);
        rt.frame(0.016);
        let pressed = rt.state.seen.iter().find(|i| matches!(i, Input::Press(_)));
        assert_eq!(
            pressed,
            Some(&Input::Press(Point { x: 150, y: 60 })),
            "at {window:?}"
        );
    }
    Ok(())
}

/// A frame Chrome wrote: a 40 by 30 page of four colour blocks, by headless Chrome's screenshot.
const QUAD: &[u8] = include_bytes!("quad.png");
/// A 400 by 240 page at 2x: a gradient, text and bars, by headless Chrome's screenshot.
const PAGE: &[u8] = include_bytes!("page.png");

#[test]
fn a_chrome_frame_decodes_to_the_colours_it_shows() -> Result<(), Box<dyn Error>> {
    let frame = decode(QUAD)?;
    assert_eq!((frame.width, frame.height), (40, 30));
    let at = |x: usize, y: usize| frame.rgba.get((y * 40 + x) * 4..(y * 40 + x) * 4 + 4);
    assert_eq!(at(5, 5), Some(&[255, 0, 0, 255][..]));
    assert_eq!(at(30, 5), Some(&[0, 255, 0, 255][..]));
    assert_eq!(at(5, 20), Some(&[0, 0, 255, 255][..]));
    assert_eq!(at(30, 20), Some(&[255, 255, 255, 255][..]));
    Ok(())
}

/// The reference is the same file turned to BMP by macOS `sips`, its RGB bytes hashed (FNV-1a).
#[test]
fn a_life_sized_frame_matches_a_second_decoder_byte_for_byte() -> Result<(), Box<dyn Error>> {
    let frame = decode(PAGE)?;
    assert_eq!((frame.width, frame.height), (400, 240));
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for pixel in frame.rgba.as_chunks::<4>().0 {
        for byte in pixel.iter().take(3) {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(hash, 0x54f5_b574_23d6_412c);
    Ok(())
}

#[test]
fn a_cut_or_damaged_frame_is_refused_not_read_past() -> Result<(), Box<dyn Error>> {
    let half = PAGE.get(..PAGE.len() / 2).ok_or("short fixture")?;
    let screen = Screen::default();
    assert!(screen.push_png(half).is_err());
    let mut flipped = PAGE.to_vec();
    if let Some(byte) = flipped.get_mut(PAGE.len() / 2) {
        *byte ^= 0xff;
    }
    let _ = screen.push_png(&flipped);
    let gif = screen.push_png(b"GIF89a");
    assert!(matches!(gif, Err(BadPicture::Damaged(_))), "{gif:?}");
    screen.push_png(QUAD)?;
    assert_eq!(screen.size(), Some((40, 30)));
    Ok(())
}

/// A frame that is only its header: 8-bit RGBA of `width` by `height`, with a true checksum.
fn claims(width: u32, height: u32) -> Vec<u8> {
    let mut chunk = b"IHDR".to_vec();
    chunk.extend(width.to_be_bytes());
    chunk.extend(height.to_be_bytes());
    chunk.extend([8, 6, 0, 0, 0]);
    let mut crc = u32::MAX;
    for byte in &chunk {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\r".to_vec();
    png.extend(chunk);
    png.extend((!crc).to_be_bytes());
    png
}

/// The cap is on what the header claims, so nothing is allocated for a frame past it: one at
/// the cap is read on (and found cut short), one row past it is refused for its size.
#[test]
fn a_frame_one_row_past_the_pixel_cap_is_refused_for_its_size() -> Result<(), Box<dyn Error>> {
    let rows = u32::try_from(MAX_PIXELS / u64::from(MAX_SIDE))?;
    assert_eq!(u64::from(MAX_SIDE) * u64::from(rows), MAX_PIXELS);
    let at = decode(&claims(MAX_SIDE, rows));
    assert!(
        matches!(at, Err(BadPicture::Damaged(_))),
        "at the cap: {at:?}"
    );
    let (width, height) = (MAX_SIDE, rows + 1);
    let past = decode(&claims(width, height));
    assert_eq!(past, Err(BadPicture::Size { width, height }));
    let (width, height) = (MAX_SIDE + 1, 1);
    let wide = decode(&claims(width, height));
    assert_eq!(wide, Err(BadPicture::Size { width, height }));
    Ok(())
}
