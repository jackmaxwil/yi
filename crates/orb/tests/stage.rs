use std::error::Error;
use std::time::Duration;

use yi_orb::stage::{blend, matching, pairings_solved};
use yi_orb::{Orb, OrbFrame, OrbState, Point, pose, render};
#[path = "support/frames.rs"]
mod support;

const STEP: f64 = 0.0625;
/// Reading leaves at every quarter of its eight-second loop: asked at once it exits at 2 s, asked
/// after 2.5 s of play it exits at 4 s.
const SOON: (f64, f64) = (0.0, 2.0);
const LATER: (f64, f64) = (2.5, 4.0);

/// Every frame an orb showing Reading draws from its request for `to`, made after `play` seconds
/// of the loop, until it settles.
fn settle((play, _): (f64, f64), to: OrbState) -> Result<Vec<OrbFrame>, String> {
    let (mut orb, dt) = (
        Orb::showing(Some(OrbState::Reading)),
        Duration::from_secs_f64(STEP),
    );
    for _ in 0..(play / STEP) as usize {
        let _ = orb.frame(dt, Some(OrbState::Reading));
    }
    let mut frames = Vec::new();
    for _ in 0..30_000 {
        frames.push(orb.frame(dt, Some(to)));
        if orb.holding(to) {
            return Ok(frames);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err("the pairing never landed".to_string())
}

/// The morph's seven inner frames as a lone solver pairs them, with no orb and no table.
fn solo((_, exit): (f64, f64), to: OrbState) -> Vec<OrbFrame> {
    let (from, target) = (pose(Some(OrbState::Reading), exit), pose(Some(to), 0.0));
    let perm = matching(&from, &target);
    (1..8)
        .map(|k| render(&blend(&from, &target, &perm, f64::from(k) * STEP / 0.5)))
        .collect()
}

fn drew(frames: &[OrbFrame], morph: &[OrbFrame]) -> bool {
    morph
        .iter()
        .all(|want| frames.iter().any(|got| support::same(got, want)))
}

#[test]
fn an_exit_and_target_are_solved_once_however_many_avatars_ask() -> Result<(), Box<dyn Error>> {
    use OrbState::{Editing, Executing, Searching};
    let before = pairings_solved();
    for (when, to, avatars, solves, why) in [
        (
            SOON,
            Searching,
            8,
            1,
            "eight avatars asking at once pay for one solve",
        ),
        (SOON, Editing, 1, 2, "a new target is one solve"),
        (
            SOON,
            Editing,
            1,
            2,
            "the next avatar finds it finished and solves none",
        ),
        (SOON, Executing, 1, 3, "another target is another solve"),
        (
            LATER,
            Editing,
            1,
            4,
            "leaving the loop at 4 s instead of 2 s is another solve",
        ),
    ] {
        let runs = std::thread::scope(|scope| {
            let asked: Vec<_> = (0..avatars)
                .map(|_| scope.spawn(move || settle(when, to)))
                .collect();
            asked
                .into_iter()
                .map(|run| run.join().map_err(|_| "an avatar panicked".to_string())?)
                .collect::<Result<Vec<_>, String>>()
        })?;
        assert_eq!(pairings_solved() - before, solves, "{why}");
        let morph = solo(when, to);
        assert!(
            runs.iter().all(|run| drew(run, &morph)),
            "{why}: every avatar draws the morph a lone solver pairs"
        );
    }
    Ok(())
}

fn gap(a: &Point, b: &Point) -> f64 {
    (0..3).map(|k| (a.at[k] - b.at[k]).powi(2)).sum()
}

/// The cheapest assignment by dynamic programming over subsets: no Hungarian method in it.
fn cheapest(from: &[Point], target: &[Point]) -> f64 {
    let n = from.len();
    let mut best = vec![f64::INFINITY; 1 << n];
    best[0] = 0.0;
    for mask in 0..(1usize << n) - 1 {
        let row = mask.count_ones() as usize;
        for (j, a) in from.iter().enumerate().filter(|(j, _)| mask >> j & 1 == 0) {
            let next = mask | 1 << j;
            best[next] = best[next].min(best[mask] + gap(a, &target[row]));
        }
    }
    best[(1 << n) - 1]
}

#[test]
fn the_pairing_an_orb_shares_is_the_cheapest_on_sixteen_points() {
    let pick =
        |points: Vec<Point>| -> Vec<Point> { points.into_iter().step_by(34).take(16).collect() };
    let from = pick(pose(Some(OrbState::Reading), SOON.1));
    let target = pick(pose(Some(OrbState::Searching), 0.0));
    // The resting mark is its visible points and invisible spares sitting on them, so a sample
    // holding each point beside its twin has ties for the assignment to break.
    let mark = pose(None, 0.0);
    let visible = mark.iter().take_while(|p| p.alpha > 0.0).count();
    let twins: Vec<Point> = (0..8)
        .flat_map(|k| [mark[k * 8], mark[visible + k * 8]])
        .collect();
    for (from, target) in [(from, target.clone()), (twins, target)] {
        let perm = matching(&from, &target);
        let paired: f64 = target
            .iter()
            .zip(&perm)
            .map(|(b, &j)| gap(&from[j], b))
            .sum();
        let least = cheapest(&from, &target);
        assert!(
            (paired - least).abs() < 1e-9,
            "paired {paired} against the cheapest {least}"
        );
    }
}
