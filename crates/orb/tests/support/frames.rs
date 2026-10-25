use yi_orb::OrbFrame;

pub fn same(a: &OrbFrame, b: &OrbFrame) -> bool {
    a.dots.len() == b.dots.len()
        && a.dots.iter().zip(&b.dots).all(|(p, q)| {
            (p.x - q.x).abs() < 1e-6
                && (p.y - q.y).abs() < 1e-6
                && (p.r - q.r).abs() < 1e-6
                && (p.a - q.a).abs() < 1e-6
        })
}
