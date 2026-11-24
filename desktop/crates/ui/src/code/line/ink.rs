//! The lines drawn under code: a wavy one for a problem, a dotted one for a name that can be
//! followed. Points are in the text's own space, from its top-left corner.

/// Points of a wave along `y` from `x0` to `x1`: one point up and down every four across.
pub fn wave(x0: f32, x1: f32, y: f32) -> Vec<[f32; 2]> {
    const STEP: f32 = 0.5;
    // Invariant: a squiggle is at most 4096 points (2048 points across), however long the line.
    let across = (0u16..4096)
        .map(|i| x0 + f32::from(i) * STEP)
        .take_while(|x| *x < x1);
    let mut points: Vec<[f32; 2]> = across
        .map(|x| [x, y + ((x - x0) * std::f32::consts::FRAC_PI_2).sin()])
        .collect();
    if points.len() > 1 {
        points.push([x1, y + ((x1 - x0) * std::f32::consts::FRAC_PI_2).sin()]);
    }
    points
}

/// The left end of each one-point dot of a dotted line from `x0` to `x1`, a point apart.
pub fn dots(x0: f32, x1: f32) -> impl Iterator<Item = f32> {
    (0u16..2048)
        .map(move |i| x0 + f32::from(i) * 2.0)
        .take_while(move |x| *x + 1.0 <= x1)
}

#[cfg(test)]
mod tests {
    use super::{dots, wave};

    type Check = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_wave_spans_its_range_and_stays_within_a_point_of_its_line() -> Check {
        let points = wave(10.0, 73.2, 14.0);
        assert_eq!(points.first(), Some(&[10.0, 14.0]));
        assert_eq!(points.last().map(|p| p[0]), Some(73.2));
        assert!(points.iter().all(|p| (13.0..=15.0).contains(&p[1])));
        assert!(points.iter().any(|p| p[1] > 14.9) && points.iter().any(|p| p[1] < 13.1));
        assert!(wave(5.0, 5.0, 14.0).is_empty());
        Ok(())
    }

    #[test]
    fn dots_stay_inside_their_range() -> Check {
        assert_eq!(
            dots(10.0, 17.5).collect::<Vec<_>>(),
            [10.0, 12.0, 14.0, 16.0]
        );
        assert_eq!(dots(10.0, 10.5).count(), 0);
        Ok(())
    }
}
