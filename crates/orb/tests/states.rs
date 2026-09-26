use std::error::Error;
use std::time::Duration;

use yi_orb::stage::{blend, matching};
use yi_orb::states::spec;
use yi_orb::{Orb, OrbFrame, OrbState, Point, pose, render};

type TestResult = Result<(), Box<dyn Error>>;

fn every_state() -> Vec<Option<OrbState>> {
    vec![
        None,
        Some(OrbState::Awaiting),
        Some(OrbState::Thinking),
        Some(OrbState::Composing),
        Some(OrbState::Working),
        Some(OrbState::Reading),
        Some(OrbState::Searching),
        Some(OrbState::Browsing),
        Some(OrbState::Editing),
        Some(OrbState::Executing),
        Some(OrbState::Computing),
        Some(OrbState::Planning { done: 3, total: 6 }),
        Some(OrbState::Delegating { children: 3 }),
        Some(OrbState::Listening),
    ]
}

fn visible(point: &Point) -> bool {
    point.alpha > 0.02
}

fn distance(a: &Point, b: &Point) -> f64 {
    (0..3)
        .map(|k| (a.at[k] - b.at[k]).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// What a viewer sees change between two frames: every visible point's nearest visible point
/// in the other frame, by position and by opacity. Which point is which cannot be seen.
fn seen_step(a: &[Point], b: &[Point]) -> (f64, f64) {
    let one_way = |x: &[Point], y: &[Point]| {
        x.iter()
            .filter(|p| visible(p))
            .fold((0.0f64, 0.0f64), |acc, p| {
                let nearest = y
                    .iter()
                    .filter(|q| visible(q))
                    .min_by(|m, n| distance(p, m).total_cmp(&distance(p, n)));
                nearest.map_or(acc, |q| {
                    (
                        acc.0.max(distance(p, q)),
                        acc.1.max((p.alpha - q.alpha).abs()),
                    )
                })
            })
    };
    let (ab, ba) = (one_way(a, b), one_way(b, a));
    (ab.0.max(ba.0), ab.1.max(ba.1))
}

fn same(a: &OrbFrame, b: &OrbFrame) -> bool {
    a.dots.len() == b.dots.len()
        && a.dots.iter().zip(&b.dots).all(|(p, q)| {
            (p.x - q.x).abs() < 1e-6
                && (p.y - q.y).abs() < 1e-6
                && (p.r - q.r).abs() < 1e-6
                && (p.a - q.a).abs() < 1e-6
        })
}

#[test]
fn every_state_loops_without_a_seam() -> TestResult {
    for state in every_state() {
        let (secs, _) = spec(state);
        let steps = (secs * 30.0).round() as usize;
        let poses: Vec<Vec<Point>> = (0..=steps)
            .map(|i| pose(state, secs * i as f64 / steps as f64))
            .collect();
        let (mut moved, mut faded) = (0.0f64, 0.0f64);
        for pair in poses.windows(2).take(steps - 1) {
            if let [a, b] = pair {
                for (p, q) in a.iter().zip(b).filter(|(p, q)| visible(p) || visible(q)) {
                    moved = moved.max(distance(p, q));
                    faded = faded.max((p.alpha - q.alpha).abs());
                }
            }
        }
        let (last, first) = (
            poses.get(steps - 1).ok_or("no last frame")?,
            poses.first().ok_or("no first frame")?,
        );
        let (seam_move, seam_fade) = seen_step(last, first);
        assert!(
            seam_move <= moved * 1.05 + 1e-9 && seam_fade <= faded * 1.05 + 1e-9,
            "{state:?} jumps where its loop wraps: moves {seam_move:.4} (loop's largest step {moved:.4}), \
             fades {seam_fade:.3} (largest {faded:.3})"
        );
    }
    Ok(())
}

#[test]
fn a_change_waits_for_the_next_exit_then_lands_on_the_entry_pose() -> TestResult {
    let step = 0.0625;
    let dt = Duration::from_secs_f64(step);
    let reading = |t: f64| render(&pose(Some(OrbState::Reading), t));
    let mut orb = Orb::showing(Some(OrbState::Reading));
    let mut clock = 0.0;
    while clock < 24.5 {
        clock += step;
        let frame = orb.frame(dt, Some(OrbState::Reading));
        assert!(
            same(&frame, &reading(clock)),
            "reading plays on at {clock} s"
        );
    }
    let exit = (clock / 2.0).ceil() * 2.0;
    loop {
        let frame = orb.frame(dt, Some(OrbState::Searching));
        if clock + step >= exit {
            break;
        }
        clock += step;
        assert!(
            same(&frame, &reading(clock)),
            "a requested change waits for the exit at {exit} s; at {clock} s the loop still plays"
        );
    }
    // The pairing runs on its own thread; until it lands the orb holds the exit pose.
    let held = reading(exit);
    let mut waited = 0;
    let mut frame = orb.frame(dt, Some(OrbState::Searching));
    while same(&frame, &held) {
        waited += 1;
        assert!(waited < 30_000, "the pairing never landed");
        std::thread::sleep(Duration::from_millis(1));
        frame = orb.frame(dt, Some(OrbState::Searching));
    }
    let mut morph = step;
    while morph + step < 0.5 {
        morph += step;
        let _ = orb.frame(dt, Some(OrbState::Searching));
    }
    let mut since = morph + step - 0.5;
    for _ in 0..16 {
        let frame = orb.frame(dt, Some(OrbState::Searching));
        assert!(
            same(&frame, &render(&pose(Some(OrbState::Searching), since))),
            "after the morph the searching loop plays from its entry pose"
        );
        since += step;
    }
    Ok(())
}

#[test]
fn a_long_gap_between_frames_advances_the_orb_one_step() {
    let (mut idle, mut busy) = (
        Orb::showing(Some(OrbState::Thinking)),
        Orb::showing(Some(OrbState::Thinking)),
    );
    // A surface that stopped repainting can hand the orb minutes at once.
    let after_gap = idle.frame(Duration::from_secs(90), Some(OrbState::Thinking));
    let after_step = busy.frame(Duration::from_millis(100), Some(OrbState::Thinking));
    assert!(
        same(&after_gap, &after_step),
        "a 90 s gap moves the orb no further than one step"
    );
}

#[test]
fn the_rest_pose_spells_the_mark() {
    // The wordmark's five strokes in its own unit square, y up: the Y's arms and stem, the
    // i's stem and its dot. On the 64-unit canvas the square spans 25.6 units either side.
    let strokes = [
        ((-0.62, 0.62), (-0.28, 0.06)),
        ((0.06, 0.62), (-0.28, 0.06)),
        ((-0.28, 0.06), (-0.28, -0.62)),
        ((0.46, 0.30), (0.46, -0.62)),
        ((0.46, 0.60), (0.46, 0.56)),
    ];
    let to_canvas = |(x, y): (f64, f64)| (32.0 + x * 25.6, 32.0 - y * 25.6);
    let off_stroke = |px: f64, py: f64| {
        strokes
            .iter()
            .map(|&(a, b)| {
                let ((ax, ay), (bx, by)) = (to_canvas(a), to_canvas(b));
                let (dx, dy) = (bx - ax, by - ay);
                let f = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy).max(1e-9))
                    .clamp(0.0, 1.0);
                (px - ax - f * dx).hypot(py - ay - f * dy)
            })
            .fold(f64::INFINITY, f64::min)
    };
    let rest = render(&pose(None, 0.0));
    assert!(
        rest.dots.len() > 40,
        "the mark has body: {}",
        rest.dots.len()
    );
    for dot in &rest.dots {
        assert!(
            off_stroke(dot.x, dot.y) < 0.5,
            "every dot of the resting mark sits on a stroke of Yi; ({:.1}, {:.1}) does not",
            dot.x,
            dot.y
        );
    }
}

#[test]
fn a_morph_moves_every_point_and_duplicates_none() -> TestResult {
    let from = pose(Some(OrbState::Thinking), 0.0);
    let target = pose(Some(OrbState::Delegating { children: 2 }), 0.0);
    let perm = matching(&from, &target);
    let mut seen = perm.clone();
    seen.sort_unstable();
    assert!(
        seen.iter().copied().eq(0..from.len()),
        "every point is used exactly once"
    );
    let travel = |perm: &[usize]| -> f64 {
        target
            .iter()
            .zip(perm)
            .map(|(b, &j)| from.get(j).map_or(0.0, |a| distance(a, b).powi(2)))
            .sum()
    };
    let identity: Vec<usize> = (0..from.len()).collect();
    assert!(
        travel(&perm) <= travel(&identity),
        "the pairing is no longer than taking points in order"
    );
    let start = blend(&from, &target, &perm, 0.0);
    let end = blend(&from, &target, &perm, 1.0);
    for ((s, e), (b, &j)) in start.iter().zip(&end).zip(target.iter().zip(&perm)) {
        let a = from.get(j).ok_or("pairing out of range")?;
        assert!(distance(s, a) < 1e-12, "a morph starts on the shown pose");
        assert!(distance(e, b) < 1e-12, "a morph ends on the entry pose");
    }
    Ok(())
}
