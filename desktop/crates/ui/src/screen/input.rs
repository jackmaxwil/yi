//! Pointer and keys inside a screen, as points of the picture, and what a source is asked.

/// The keys that type nothing, as a source is told them.
pub use yd_core::feed::ScreenKey as Named;
use yd_core::feed::{Pointer, ScreenInput};

/// A point of the picture, in its own pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

/// What happened inside a screen; every point is already a point of the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Press(Point),
    Move(Point),
    Release(Point),
    /// Points to scroll by at the pointer, positive moving the content right and down.
    Wheel {
        dx: i32,
        dy: i32,
    },
    Text(String),
    Key(Named),
    /// The box the picture is fitted to, in points: what the far side should draw at.
    Resized {
        width: u32,
        height: u32,
    },
}

/// The key a screen passes on for `key`; none for one that types or that a screen ignores.
pub fn named(key: &rune::Key) -> Option<Named> {
    Some(match key {
        rune::Key::Enter => Named::Enter,
        rune::Key::Backspace => Named::Backspace,
        rune::Key::Delete => Named::Delete,
        rune::Key::Tab => Named::Tab,
        rune::Key::Escape => Named::Escape,
        rune::Key::Left => Named::Left,
        rune::Key::Right => Named::Right,
        rune::Key::Up => Named::Up,
        rune::Key::Down => Named::Down,
        _ => return None,
    })
}

/// Where a picture of `size` pixels lands in a box of `w` by `h` points, fitted whole and
/// held to the top-left: the points per pixel, none for an empty picture or box.
pub fn scale(size: (u32, u32), w: f32, h: f32) -> Option<f32> {
    let (pw, ph) = (size.0 as f32, size.1 as f32);
    let s = (w / pw).min(h / ph);
    (pw > 0.0 && ph > 0.0 && s.is_finite() && s > 0.0).then_some(s)
}

/// The picture's pixel under a point of the box, none when the point is off the picture.
pub fn to_picture(size: (u32, u32), (w, h): (f32, f32), (x, y): (f32, f32)) -> Option<Point> {
    let s = scale(size, w, h)?;
    let (px, py) = (x / s, y / s);
    let inside = |v: f32, n: u32| v >= 0.0 && v < n as f32;
    (inside(px, size.0) && inside(py, size.1)).then(|| Point {
        x: px.floor() as u32,
        y: py.floor() as u32,
    })
}

impl Input {
    /// What a source is asked for this input; none for the box's size, which is a resize.
    pub fn ask(self) -> Option<ScreenInput> {
        let pointer = |phase, at: Point| ScreenInput::Pointer {
            phase,
            x: at.x,
            y: at.y,
        };
        Some(match self {
            Input::Press(at) => pointer(Pointer::Press, at),
            Input::Move(at) => pointer(Pointer::Move, at),
            Input::Release(at) => pointer(Pointer::Release, at),
            Input::Wheel { dx, dy } => ScreenInput::Wheel { dx, dy },
            Input::Text(text) => ScreenInput::Text(text),
            Input::Key(key) => ScreenInput::Key(key),
            Input::Resized { .. } => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{Point, to_picture};

    #[test]
    fn a_point_off_the_fitted_picture_is_not_a_page_point() -> Result<(), Box<dyn Error>> {
        let size = (200, 100);
        assert_eq!(
            to_picture(size, (400.0, 300.0), (399.0, 199.0)),
            Some(Point { x: 199, y: 99 })
        );
        assert_eq!(to_picture(size, (400.0, 300.0), (100.0, 201.0)), None);
        assert_eq!(to_picture((0, 0), (400.0, 300.0), (1.0, 1.0)), None);
        Ok(())
    }
}
