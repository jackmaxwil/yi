// thinking-orbs mode painters, port verbatim (A.13): orbits (working),
// globe/rubik/wave (searching/solving/listening), web (connecting),
// braid (weaving), ribbon/ring (composing/breathing), morph (shaping).

use std::f64::consts::PI;

use crate::Opts;
use crate::core::{
    Dot, OrbFrame, OrbLine, angle_delta, fib_dir, finalize_frame, frac, hash_d, lerp, make_proj,
    radius_scale, vnoise,
};

pub fn frame_orbits(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.82;
    let pt = make_proj(t * 0.12, 0.3, cx, cy, 1.0);
    let rs = radius_scale(size, o.get("rsPow", 0.6));

    let mut dots = Vec::new();
    let orbit_n = o.get("orbitN", 12.0) as usize;
    let ghost_n = o.get("ghostN", 40.0) as usize;
    let particles = o.get("particles", 3.0) as usize;

    for orb in 0..orbit_n {
        let orb_f = orb as f64;
        let h1 = hash_d(orb_f, 1.7);
        let h2 = hash_d(orb_f, 5.2);
        let h3 = hash_d(orb_f, 8.9);
        let ro = big_r * (0.45 + 0.52 * h1);
        let th = h1 * 2.0 * PI;
        let phi = (2.0 * h2 - 1.0).acos();
        let nx = phi.sin() * th.cos();
        let ny = phi.cos();
        let nz = phi.sin() * th.sin();
        let mut ux = -ny;
        let mut uy = nx;
        let uz = 0.0;
        let ul = (ux * ux + uy * uy).sqrt().max(1e-6);
        ux /= ul;
        uy /= ul;
        let vx = ny * uz - nz * uy;
        let vy = nz * ux - nx * uz;
        let vz = nx * uy - ny * ux;
        let speed = (0.25 + 0.55 * h3) * if h3 > 0.5 { 1.0 } else { -1.0 };

        for k in 0..ghost_n {
            let a = (k as f64 / ghost_n as f64) * 2.0 * PI;
            let (px, py, z) = pt.point(
                (ux * a.cos() + vx * a.sin()) * ro,
                (uy * a.cos() + vy * a.sin()) * ro,
                (uz * a.cos() + vz * a.sin()) * ro,
            );
            let depth = (z / ro + 1.0) / 2.0;
            dots.push(Dot {
                x: px,
                y: py,
                z,
                r: o.get("ghostR", 0.9) * rs,
                white: 0.72,
                a: o.get("ghostA", 0.5) * (0.4 + 0.6 * depth),
            });
        }
        for m in 0..particles {
            let a = t * speed + (m as f64 / particles as f64) * 2.0 * PI + h2 * 6.0;
            let (px, py, z) = pt.point(
                (ux * a.cos() + vx * a.sin()) * ro,
                (uy * a.cos() + vy * a.sin()) * ro,
                (uz * a.cos() + vz * a.sin()) * ro,
            );
            let depth = (z / ro + 1.0) / 2.0;
            dots.push(Dot {
                x: px,
                y: py,
                z,
                r: (o.get("partR", 1.2) + o.get("partRDepth", 1.6) * depth) * rs,
                white: 0.3 - 0.22 * depth,
                a: 1.0,
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

pub fn frame_globe(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let spin = 0.5;
    let cx = size / 2.0;
    let cy = size / 2.0;
    let radius = (size / 2.0) * 0.82;
    let tilt = 0.4 + 0.06 * (t * 0.35).sin();
    let pt = make_proj(t * spin, tilt, cx, cy, radius);
    let scan = t * (spin + (1.7 - spin) * o.get("scanMul", 1.0));
    let rs = radius_scale(size, o.get("rsPow", 0.6));
    let dim_base = o.get("dimBase", 1.0);

    let mut dots = Vec::new();
    let lat_rings = o.get("latRings", 17.0) as i64;
    let lon_density = o.get("lonDensity", 44.0);
    for li in 0..=lat_rings {
        let lat = -PI / 2.0 + (li as f64 / lat_rings as f64) * PI;
        let cos_lat = lat.cos();
        let sin_lat = lat.sin();
        let lon_count = ((cos_lat.abs() * lon_density).round() as i64).max(1);
        for lj in 0..lon_count {
            let lon = (lj as f64 / lon_count as f64) * 2.0 * PI;
            let (px, py, z) = pt.point(cos_lat * lon.cos(), sin_lat, cos_lat * lon.sin());
            let depth = (z + 1.0) / 2.0;
            let d = angle_delta(lon + t * spin, scan);
            let boost = (-(d * d) / 0.18).exp() * z.max(0.0);
            dots.push(Dot {
                x: px,
                y: py,
                z,
                r: (o.get("rBase", 0.6)
                    + o.get("rDepth", 1.7) * depth
                    + o.get("rBoost", 1.0) * boost)
                    * rs,
                white: o.get("inkFar", 0.62) - o.get("inkSpan", 0.54) * depth,
                a: dim_base + (1.0 - dim_base) * boost.min(1.0),
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

struct Move {
    axis: u8,
    lo: f64,
    hi: f64,
    ang: f64,
}

fn make_moves(count: usize) -> Vec<Move> {
    (0..count)
        .map(|i| {
            let i_f = i as f64;
            let axis = ((hash_d(i_f, 2.3) * 3.0).floor() as u8).min(2);
            let lo = -1.0 + 0.5 * (hash_d(i_f, 5.9) * 4.0).floor().min(3.0);
            let dir = if hash_d(i_f, 7.7) < 0.5 { 1.0 } else { -1.0 };
            Move {
                axis,
                lo,
                hi: lo + 0.5,
                ang: dir * PI / 2.0,
            }
        })
        .collect()
}

fn solve_cycle(time: f64, count: usize, slot_dur: f64, rest: f64) -> (Vec<f64>, i64) {
    let count_f = count as f64;
    let cyc = 2.0 * count_f * slot_dur + rest;
    let tc = time % cyc;
    let mut amount = vec![0.0; count];
    let mut active: i64 = -1;
    if tc < 2.0 * count_f * slot_dur {
        let slot = (tc / slot_dur).floor() as usize;
        let p = (tc - slot as f64 * slot_dur) / slot_dur;
        let cl = (p / 0.7).min(1.0);
        let ep = 1.0 - (1.0 - cl).powi(3);
        if slot < count {
            for a in amount.iter_mut().take(slot) {
                *a = 1.0;
            }
            amount[slot] = ep;
            active = slot as i64;
        } else {
            let u = 2 * count - 1 - slot;
            for a in amount.iter_mut().take(u) {
                *a = 1.0;
            }
            amount[u] = 1.0 - ep;
            active = u as i64;
        }
    }
    (amount, active)
}

fn apply_moves(
    mut x: f64,
    mut y: f64,
    mut z: f64,
    moves: &[Move],
    amount: &[f64],
    active: i64,
) -> (f64, f64, f64, bool) {
    let mut in_active = false;
    for (i, mv) in moves.iter().enumerate() {
        let amt = amount.get(i).copied().unwrap_or(0.0);
        if amt <= 0.0 {
            continue;
        }
        let coord = match mv.axis {
            0 => x,
            1 => y,
            _ => z,
        };
        if coord < mv.lo || coord >= mv.hi {
            continue;
        }
        if i as i64 == active {
            in_active = true;
        }
        let a = mv.ang * amt;
        let ca = a.cos();
        let sa = a.sin();
        match mv.axis {
            0 => {
                let y2 = y * ca - z * sa;
                z = y * sa + z * ca;
                y = y2;
            }
            1 => {
                let x2 = x * ca + z * sa;
                z = -x * sa + z * ca;
                x = x2;
            }
            _ => {
                let x2 = x * ca - y * sa;
                y = x * sa + y * ca;
                x = x2;
            }
        }
    }
    (x, y, z, in_active)
}

pub fn frame_rubik(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.82;
    let pt = make_proj(t * 0.55, 0.35 + 0.1 * (t * 0.9).sin(), cx, cy, big_r);
    let rs = radius_scale(size, o.get("rsPow", 0.6));
    let move_count = o.get("moveCount", 14.0) as usize;
    let moves = make_moves(move_count);
    let (amount, active) = solve_cycle(t, move_count, 0.42, 1.2);

    let mut dots = Vec::new();
    let lat_rings = o.get("latRings", 15.0) as i64;
    let lon_density = o.get("lonDensity", 40.0);
    for li in 0..=lat_rings {
        let lat = -PI / 2.0 + (li as f64 / lat_rings as f64) * PI;
        let cos_lat = lat.cos();
        let sin_lat = lat.sin();
        let lon_count = ((cos_lat.abs() * lon_density).round() as i64).max(1);
        for lj in 0..lon_count {
            let lon = (lj as f64 / lon_count as f64) * 2.0 * PI;
            let (x, y, z, in_active) = apply_moves(
                cos_lat * lon.cos(),
                sin_lat,
                cos_lat * lon.sin(),
                &moves,
                &amount,
                active,
            );
            let (px, py, zr) = pt.point(x, y, z);
            let depth = (zr + 1.0) / 2.0;
            dots.push(Dot {
                x: px,
                y: py,
                z: zr,
                r: (o.get("rBase", 0.6)
                    + o.get("rDepth", 1.7) * depth
                    + if in_active {
                        o.get("rActive", 0.3)
                    } else {
                        0.0
                    })
                    * rs,
                white: o.get("inkFar", 0.62)
                    - o.get("inkSpan", 0.54) * depth
                    - if in_active { 0.14 } else { 0.0 },
                a: 1.0,
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

pub fn frame_wave(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.874;
    let pt = make_proj(t * 0.18, 0.38, cx, cy, 1.0);
    let rs = radius_scale(size, o.get("rsPow", 0.6));

    let mut dots = Vec::new();
    let rings = o.get("rings", 15.0) as i64;
    let lon_density = o.get("lonDensity", 40.0);
    for ri in 0..=rings {
        let lat = -PI / 2.0 + (ri as f64 / rings as f64) * PI;
        let cos_lat = lat.cos();
        let sin_lat = lat.sin();
        let w =
            0.62 * (t * 2.1 - ri as f64 * 0.52).sin() + 0.38 * (t * 1.27 + ri as f64 * 0.83).sin();
        let rr = big_r * (0.88 + 0.105 * w);
        let lon_count = ((cos_lat.abs() * lon_density).round() as i64).max(1);
        for lj in 0..lon_count {
            let lon = (lj as f64 / lon_count as f64) * 2.0 * PI;
            let (px, py, z) = pt.point(
                cos_lat * lon.cos() * rr,
                sin_lat * rr,
                cos_lat * lon.sin() * rr,
            );
            let depth = (z / big_r + 1.0) / 2.0;
            let crest = w.max(0.0);
            dots.push(Dot {
                x: px,
                y: py,
                z,
                r: (o.get("rBase", 0.6) + o.get("rDepth", 1.7) * depth) * (1.0 + 0.4 * crest) * rs,
                white: 0.66 - 0.56 * depth - 0.1 * crest,
                a: 1.0,
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

pub fn frame_web(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.8 * o.get("spread", 1.0);
    let pt = make_proj(t * 0.12, 0.32, cx, cy, big_r);
    let rs = radius_scale(size, o.get("rsPow", 0.6));

    let node_n = o.get("nodeN", 30.0) as usize;
    let thr = o.get("thr", 0.72);
    let node_r = o.get("nodeR", 1.4);
    let node_r_depth = o.get("nodeRDepth", 1.8);

    let mut nodes: Vec<(f64, f64, f64)> = Vec::with_capacity(node_n);
    for i in 0..node_n {
        let i_f = i as f64;
        let d = fib_dir(i_f, node_n as f64);
        let x = d.0 + 0.3 * (vnoise(i_f * 0.31 + 9.0, t * 0.24) - 0.5) * 2.0;
        let y = d.1 + 0.3 * (vnoise(i_f * 0.53 + 27.0, t * 0.21) - 0.5) * 2.0;
        let z = d.2 + 0.3 * (vnoise(i_f * 0.77 + 55.0, t * 0.27) - 0.5) * 2.0;
        let l = (x * x + y * y + z * z).sqrt();
        nodes.push((x / l, y / l, z / l));
    }

    let mut lines = Vec::new();
    let mut dots = Vec::new();

    for i in 0..node_n {
        for j in (i + 1)..node_n {
            let dx = nodes[i].0 - nodes[j].0;
            let dy = nodes[i].1 - nodes[j].1;
            let dz = nodes[i].2 - nodes[j].2;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            if dist >= thr {
                continue;
            }
            let (x1, y1, z1) = pt.point(nodes[i].0, nodes[i].1, nodes[i].2);
            let (x2, y2, z2) = pt.point(nodes[j].0, nodes[j].1, nodes[j].2);
            let depth = ((z1 + z2) / 2.0 + 1.0) / 2.0;
            lines.push(OrbLine {
                x1,
                y1,
                x2,
                y2,
                white: 0.42,
                a: (1.0 - dist / thr) * (0.3 + 0.55 * depth),
                w: (o.get("lineW", 0.8) * rs).max(0.6),
            });
        }
    }

    for (i, node) in nodes.iter().enumerate() {
        let (px, py, z) = pt.point(node.0, node.1, node.2);
        let depth = (z + 1.0) / 2.0;
        let pulse = 1.0 + 0.25 * (t * 1.4 + i as f64 * 2.7).sin();
        dots.push(Dot {
            x: px,
            y: py,
            z,
            r: (node_r + node_r_depth * depth) * pulse * rs,
            white: 0.55 - 0.45 * depth,
            a: 1.0,
        });
    }

    let signals = o.get("signals", 5.0) as usize;
    for s in 0..signals {
        let s_f = s as f64;
        let seg = (t * 0.55 + s_f * 7.31).floor();
        let a = (hash_d(seg, s_f * 3.1 + 1.7) * node_n as f64).floor() as usize;
        let b = (hash_d(seg, s_f * 5.7 + 4.2) * node_n as f64).floor() as usize;
        if a == b {
            continue;
        }
        let f = frac(t * 0.55 + s_f * 7.31);
        let x = lerp(nodes[a].0, nodes[b].0, f);
        let y = lerp(nodes[a].1, nodes[b].1, f);
        let z = lerp(nodes[a].2, nodes[b].2, f);
        let l = (x * x + y * y + z * z).sqrt().max(1e-6);
        let (px, py, zr) = pt.point(x / l, y / l, z / l);
        let depth = (zr + 1.0) / 2.0;
        dots.push(Dot {
            x: px,
            y: py,
            z: zr,
            r: (node_r * 1.5 + node_r_depth * depth) * rs,
            white: 0.05,
            a: 0.5 + 0.5 * depth,
        });
    }

    finalize_frame(dots, lines, o.get("rMin", 0.3))
}

fn ghost_sphere(dots: &mut Vec<Dot>, pt: &crate::core::Proj, big_r: f64, ghost_n: usize, rs: f64) {
    for i in 0..ghost_n {
        let d = fib_dir(i as f64, ghost_n as f64);
        let (px, py, z) = pt.point(d.0 * big_r, d.1 * big_r, d.2 * big_r);
        let depth = (z / big_r + 1.0) / 2.0;
        dots.push(Dot {
            x: px,
            y: py,
            z,
            r: 0.8 * rs,
            white: 0.78,
            a: 0.1 + 0.22 * depth,
        });
    }
}

pub fn frame_braid(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.76;
    let pt = make_proj(t * 0.4, 0.3, cx, cy, 1.0);
    let rs = radius_scale(size, o.get("rsPow", 0.6));

    let mut dots = Vec::new();
    ghost_sphere(&mut dots, &pt, big_r, o.get("ghostN", 150.0) as usize, rs);

    let strand_n = o.get("strandN", 52.0) as usize;
    let turns = o.get("turns", 3.0);
    for s in 0..3 {
        let phase = (s as f64 / 3.0) * 2.0 * PI;
        for i in 0..strand_n {
            let u = (frac(i as f64 / strand_n as f64 + t * 0.045) * 2.0 - 1.0) * 0.96;
            let surf = (1.0 - u * u).max(0.0).sqrt();
            let end_fade = ((1.0 - u.abs()) / 0.1).min(1.0);
            let a = u * PI * turns + phase;
            let weave = 1.0 + 0.075 * (u * PI * turns * 2.0 + phase * 2.0 + t * 0.8).sin();
            let rr = surf * big_r * weave;
            let (px, py, zr) = pt.point(a.cos() * rr, u * big_r * weave, a.sin() * rr);
            let depth = (zr / big_r + 1.0) / 2.0;
            dots.push(Dot {
                x: px,
                y: py,
                z: zr,
                r: (o.get("rBase", 1.2) + o.get("rDepth", 1.8) * depth) * rs,
                white: 0.55 - 0.45 * depth,
                a: end_fade * (0.45 + 0.55 * depth),
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

pub fn frame_ribbon(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let big_r = (size / 2.0) * 0.78;
    let spin = o.get("spin", 1.0);
    let cam_tilt = 0.3;
    let pt = make_proj(t * 0.1 * spin, cam_tilt, cx, cy, 1.0);
    let rs = radius_scale(size, o.get("rsPow", 0.6));
    let face_on = o.get("faceOn", 0.0) != 0.0;

    let mut dots = Vec::new();
    ghost_sphere(&mut dots, &pt, big_r, o.get("ghostN", 150.0) as usize, rs);

    let ya = t * 0.24 * spin;
    let ta = if face_on {
        -cam_tilt
    } else {
        0.55 + 0.3 * (t * 0.18).sin() * spin
    };
    let ux = ya.cos();
    let uy = 0.0;
    let uz = ya.sin();
    let vx = -uz * ta.sin();
    let vy = ta.cos();
    let vz = ux * ta.sin();
    let nx = uy * vz - uz * vy;
    let ny = uz * vx - ux * vz;
    let nz = ux * vy - uy * vx;

    let wob_amp = 0.23 * o.get("wobMul", 1.0);
    let base_r = if face_on {
        big_r / (1.0 + 0.85 * wob_amp)
    } else {
        big_r
    };

    let base_lanes = o.get("lanes", 5.0);
    let segs = o.get("segs", 88.0) as usize;
    let lanes = ((base_lanes * o.get("bandMul", 1.0)).round() as i64).max(1) as usize;
    for w in 0..lanes {
        let w_f = w as f64;
        let lane_off = (w_f - (lanes as f64 - 1.0) / 2.0) * 0.075;
        let edge = (w_f - (lanes as f64 - 1.0) / 2.0).abs() / ((lanes as f64 - 1.0) / 2.0).max(1.0);
        for k in 0..segs {
            let a = (k as f64 / segs as f64) * 2.0 * PI;
            let wob = (0.16 * (a * 3.0 - t * 1.7 + w_f * 0.22).sin()
                + 0.07 * (a * 5.0 + t * 1.1).sin())
                * o.get("wobMul", 1.0);
            let radial = if face_on { 1.0 + wob } else { 1.0 };
            let off = if face_on { lane_off } else { lane_off + wob };
            let x = ux * a.cos() + vx * a.sin() + nx * off;
            let y = uy * a.cos() + vy * a.sin() + ny * off;
            let z = uz * a.cos() + vz * a.sin() + nz * off;
            let l = (x * x + y * y + z * z).sqrt();
            let rr = base_r * radial;
            let (px, py, zr) = pt.point((x / l) * rr, (y / l) * rr, (z / l) * rr);
            let depth = (zr / big_r + 1.0) / 2.0;
            dots.push(Dot {
                x: px,
                y: py,
                z: zr,
                r: (o.get("rBase", 1.1) + o.get("rDepth", 1.7) * depth) * (1.0 - 0.25 * edge) * rs,
                white: 0.52 - 0.44 * depth + 0.18 * edge,
                a: 0.4 + 0.6 * depth,
            });
        }
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.3))
}

fn smooth_e(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn poly_point(verts: &[(f64, f64)], f: f64) -> (f64, f64) {
    let v = verts.len();
    let mut lengths = Vec::with_capacity(v);
    let mut total = 0.0;
    for i in 0..v {
        let a = verts[i];
        let b = verts[(i + 1) % v];
        let l = (b.0 - a.0).hypot(b.1 - a.1);
        lengths.push(l);
        total += l;
    }
    let mut target = f * total;
    let mut i = 0;
    while target > lengths[i] && i < v - 1 {
        target -= lengths[i];
        i += 1;
    }
    let a = verts[i];
    let b = verts[(i + 1) % v];
    let ff = if lengths[i] > 0.0 {
        (target / lengths[i]).min(1.0)
    } else {
        0.0
    };
    (a.0 + (b.0 - a.0) * ff, a.1 + (b.1 - a.1) * ff)
}

const TRIANGLE: [(f64, f64); 3] = [(0.0, -0.26), (0.24, 0.16), (-0.24, 0.16)];
const SQUARE: [(f64, f64); 5] = [
    (0.0, -0.2),
    (0.2, -0.2),
    (0.2, 0.2),
    (-0.2, 0.2),
    (-0.2, -0.2),
];

fn cycle_point(k: usize, f: f64) -> (f64, f64) {
    match k % 3 {
        0 => {
            let a = -PI / 2.0 + f * 2.0 * PI;
            (a.cos() * 0.24, a.sin() * 0.24)
        }
        1 => poly_point(&TRIANGLE, f),
        _ => poly_point(&SQUARE, f),
    }
}

const HOLD: f64 = 1.4;
const MORPH: f64 = 0.9;
const SEG: f64 = HOLD + MORPH;

pub fn frame_morph(size: f64, t: f64, o: &Opts) -> OrbFrame {
    let k_count = 3.0;
    let tc = t % (SEG * k_count);
    let k = (tc / SEG).floor() as usize;
    let local = tc - k as f64 * SEG;
    let m = if local > HOLD {
        smooth_e((local - HOLD) / MORPH)
    } else {
        0.0
    };
    let sprd = o.get("spread", 1.0);

    const M: usize = 160;
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(M);
    for i in 0..M {
        let f = i as f64 / M as f64;
        let a = cycle_point(k, f);
        let b = cycle_point(k + 1, f);
        pts.push((
            (a.0 + (b.0 - a.0) * m) * sprd,
            (a.1 + (b.1 - a.1) * m) * sprd,
        ));
    }
    let mut lengths = Vec::with_capacity(M);
    let mut total = 0.0;
    for i in 0..M {
        let a = pts[i];
        let b = pts[(i + 1) % M];
        let l = (b.0 - a.0).hypot(b.1 - a.1);
        lengths.push(l);
        total += l;
    }

    let n = ((34.0 * o.get("iconD", 1.0)).round() as i64).max(6) as usize;
    let re = o.get("rDot", 0.021) * 1.35 * sprd;
    let pulse = 1.0 + 0.02 * (local * 3.1).sin();

    let mut dots = Vec::new();
    let c2 = size / 2.0;
    let mut seg = 0;
    let mut acc = 0.0;
    for k2 in 0..n {
        let target = (k2 as f64 / n as f64) * total;
        while acc + lengths[seg] < target && seg < M - 1 {
            acc += lengths[seg];
            seg += 1;
        }
        let a = pts[seg];
        let b = pts[(seg + 1) % M];
        let f = if lengths[seg] > 0.0 {
            ((target - acc) / lengths[seg]).min(1.0)
        } else {
            0.0
        };
        let x = (a.0 + (b.0 - a.0) * f) * pulse;
        let y = (a.1 + (b.1 - a.1) * f) * pulse;
        dots.push(Dot {
            x: c2 + x * size,
            y: c2 + y * size,
            z: 0.0,
            r: (re * size).max(0.35),
            white: 0.1,
            a: 1.0,
        });
    }
    finalize_frame(dots, Vec::new(), o.get("rMin", 0.25))
}
