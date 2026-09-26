// Resolved (state × size) tunings baked from the golden spec `orbs-golden.json`: the
// post-scaling numbers the golden vectors came from, so no scaling machinery can drift.

use crate::{Mode, Opts};

pub struct Resolved {
    pub mode: Mode,
    pub speed: f64,
    pub opts: &'static [(&'static str, f64)],
}

pub fn resolve(state: &str, size: u32) -> Option<&'static Resolved> {
    match (state, size) {
        ("working", 64) => Some(&WORKING_64),
        ("working", 20) => Some(&WORKING_20),
        ("searching", 64) => Some(&SEARCHING_64),
        ("searching", 20) => Some(&SEARCHING_20),
        ("solving", 64) => Some(&SOLVING_64),
        ("solving", 20) => Some(&SOLVING_20),
        ("listening", 64) => Some(&LISTENING_64),
        ("listening", 20) => Some(&LISTENING_20),
        ("connecting", 64) => Some(&CONNECTING_64),
        ("connecting", 20) => Some(&CONNECTING_20),
        ("weaving", 64) => Some(&WEAVING_64),
        ("weaving", 20) => Some(&WEAVING_20),
        ("composing", 64) => Some(&COMPOSING_64),
        ("composing", 20) => Some(&COMPOSING_20),
        ("breathing", 64) => Some(&BREATHING_64),
        ("breathing", 20) => Some(&BREATHING_20),
        ("shaping", 64) => Some(&SHAPING_64),
        ("shaping", 20) => Some(&SHAPING_20),
        _ => None,
    }
}

static WORKING_64: Resolved = Resolved {
    mode: Mode::Orbits,
    speed: 1.885_f64,
    opts: &[
        ("orbitN", 12.0_f64),
        ("ghostN", 40.0_f64),
        ("ghostR", 0.9_f64),
        ("ghostA", 0.5_f64),
        ("particles", 3.0_f64),
        ("partR", 1.2_f64),
        ("partRDepth", 1.6_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
    ],
};
static WORKING_20: Resolved = Resolved {
    mode: Mode::Orbits,
    speed: 3.9_f64,
    opts: &[
        ("orbitN", 3.0_f64),
        ("ghostN", 10.0_f64),
        ("ghostR", 2.16_f64),
        ("ghostA", 0.5_f64),
        ("particles", 3.0_f64),
        ("partR", 2.88_f64),
        ("partRDepth", 3.84_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 2.4_f64),
    ],
};
static SEARCHING_64: Resolved = Resolved {
    mode: Mode::Globe,
    speed: 2.015_f64,
    opts: &[
        ("latRings", 11.0_f64),
        ("lonDensity", 29.0_f64),
        ("rBase", 0.69_f64),
        ("rDepth", 1.9549999999999998_f64),
        ("rBoost", 1.0_f64),
        ("inkFar", 0.62_f64),
        ("inkSpan", 0.54_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.15_f64),
        ("scanMul", 4.08_f64),
        ("dimBase", 0.45_f64),
    ],
};
static SEARCHING_20: Resolved = Resolved {
    mode: Mode::Globe,
    speed: 2.665_f64,
    opts: &[
        ("latRings", 6.0_f64),
        ("lonDensity", 14.0_f64),
        ("rBase", 1.05_f64),
        ("rDepth", 2.975_f64),
        ("rBoost", 1.0_f64),
        ("inkFar", 0.62_f64),
        ("inkSpan", 0.54_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.75_f64),
        ("scanMul", 4.335_f64),
        ("dimBase", 0.45_f64),
    ],
};
static SOLVING_64: Resolved = Resolved {
    mode: Mode::Rubik,
    speed: 1.82_f64,
    opts: &[
        ("latRings", 9.0_f64),
        ("lonDensity", 24.0_f64),
        ("moveCount", 14.0_f64),
        ("rBase", 0.63_f64),
        ("rDepth", 1.785_f64),
        ("rActive", 0.315_f64),
        ("inkFar", 0.62_f64),
        ("inkSpan", 0.54_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.05_f64),
    ],
};
static SOLVING_20: Resolved = Resolved {
    mode: Mode::Rubik,
    speed: 1.95_f64,
    opts: &[
        ("latRings", 4.0_f64),
        ("lonDensity", 12.0_f64),
        ("moveCount", 14.0_f64),
        ("rBase", 1.14_f64),
        ("rDepth", 3.23_f64),
        ("rActive", 0.57_f64),
        ("inkFar", 0.62_f64),
        ("inkSpan", 0.54_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.9_f64),
    ],
};
static LISTENING_64: Resolved = Resolved {
    mode: Mode::Wave,
    speed: 4.388_f64,
    opts: &[
        ("rings", 9.0_f64),
        ("lonDensity", 23.0_f64),
        ("rBase", 0.6_f64),
        ("rDepth", 1.7_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
    ],
};
static LISTENING_20: Resolved = Resolved {
    mode: Mode::Wave,
    speed: 3.998_f64,
    opts: &[
        ("rings", 5.0_f64),
        ("lonDensity", 13.0_f64),
        ("rBase", 0.96_f64),
        ("rDepth", 2.72_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.6_f64),
    ],
};
static CONNECTING_64: Resolved = Resolved {
    mode: Mode::Web,
    speed: 3.315_f64,
    opts: &[
        ("nodeN", 41.0_f64),
        ("thr", 0.72_f64),
        ("signals", 7.0_f64),
        ("nodeR", 1.3299999999999998_f64),
        ("nodeRDepth", 1.71_f64),
        ("lineW", 0.8_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 0.95_f64),
    ],
};
static CONNECTING_20: Resolved = Resolved {
    mode: Mode::Web,
    speed: 6.63_f64,
    opts: &[
        ("nodeN", 8.0_f64),
        ("thr", 0.72_f64),
        ("signals", 1.0_f64),
        ("nodeR", 2.1279999999999997_f64),
        ("nodeRDepth", 2.736_f64),
        ("lineW", 0.8_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.52_f64),
    ],
};
static WEAVING_64: Resolved = Resolved {
    mode: Mode::Braid,
    speed: 1.625_f64,
    opts: &[
        ("strandN", 26.0_f64),
        ("turns", 3.0_f64),
        ("ghostN", 75.0_f64),
        ("rBase", 1.2_f64),
        ("rDepth", 1.8_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
    ],
};
static WEAVING_20: Resolved = Resolved {
    mode: Mode::Braid,
    speed: 2.75_f64,
    opts: &[
        ("strandN", 6.0_f64),
        ("turns", 3.0_f64),
        ("ghostN", 17.0_f64),
        ("rBase", 1.6320000000000001_f64),
        ("rDepth", 2.4480000000000004_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.36_f64),
    ],
};
static COMPOSING_64: Resolved = Resolved {
    mode: Mode::Ribbon,
    speed: 2.34_f64,
    opts: &[
        ("lanes", 3.0_f64),
        ("segs", 44.0_f64),
        ("ghostN", 38.0_f64),
        ("rBase", 0.935_f64),
        ("rDepth", 1.4449999999999998_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 0.85_f64),
        ("spin", 0.0_f64),
        ("bandMul", 3.9_f64),
        ("wobMul", 1.0_f64),
    ],
};
static COMPOSING_20: Resolved = Resolved {
    mode: Mode::Ribbon,
    speed: 3.12_f64,
    opts: &[
        ("lanes", 2.0_f64),
        ("segs", 20.0_f64),
        ("ghostN", 8.0_f64),
        ("rBase", 1.1803000000000001_f64),
        ("rDepth", 1.8240999999999998_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.073_f64),
        ("spin", 0.0_f64),
        ("bandMul", 4.94_f64),
        ("wobMul", 1.0_f64),
    ],
};
static BREATHING_64: Resolved = Resolved {
    mode: Mode::Ring,
    speed: 3.24_f64,
    opts: &[
        ("lanes", 3.0_f64),
        ("segs", 44.0_f64),
        ("ghostN", 0.0_f64),
        ("faceOn", 1.0_f64),
        ("rBase", 1.0516_f64),
        ("rDepth", 1.6252_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 0.956_f64),
        ("spin", 0.0_f64),
        ("bandMul", 3.627_f64),
        ("wobMul", 0.368_f64),
    ],
};
static BREATHING_20: Resolved = Resolved {
    mode: Mode::Ring,
    speed: 3.78_f64,
    opts: &[
        ("lanes", 2.0_f64),
        ("segs", 15.0_f64),
        ("ghostN", 0.0_f64),
        ("faceOn", 1.0_f64),
        ("rBase", 1.7842000000000002_f64),
        ("rDepth", 2.7574_f64),
        ("rsPow", 0.6_f64),
        ("rMin", 0.3_f64),
        ("rSizeMul", 1.622_f64),
        ("spin", 0.0_f64),
        ("bandMul", 3.968_f64),
        ("wobMul", 0.565_f64),
    ],
};
static SHAPING_64: Resolved = Resolved {
    mode: Mode::Morph,
    speed: 2.405_f64,
    opts: &[
        ("rDot", 0.008295_f64),
        ("iconD", 0.702_f64),
        ("rMin", 0.25_f64),
        ("rSizeMul", 0.395_f64),
        ("spread", 1.45_f64),
    ],
};
static SHAPING_20: Resolved = Resolved {
    mode: Mode::Morph,
    speed: 2.08_f64,
    opts: &[
        ("rDot", 0.021231_f64),
        ("iconD", 0.53_f64),
        ("rMin", 0.25_f64),
        ("rSizeMul", 1.011_f64),
        ("spread", 1.45_f64),
    ],
};

impl Resolved {
    pub fn options(&self) -> Opts {
        Opts(self.opts.iter().copied().collect())
    }
}
