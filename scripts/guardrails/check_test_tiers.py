#!/usr/bin/env python3
"""`just journeys` selects its lane with `--ignored` and nothing else, and the
postmerge gate runs that lane. But `#[ignore]` is Rust's marker for slow, flaky,
network-bound and paid tests alike, so the first ignored paid test would enrol
itself in a gate silently. The reason string is therefore the lane, and exact:
anything that spends money stays opt-in and user-run, reachable by neither."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

MARKER = '#[ignore = "tier-2 journey: `just journeys`"]'
IGNORE = re.compile(r"#\[ignore\b[^\]]*\]")

errs, journeys = [], 0
for path in sorted((ROOT / "crates").glob("**/*.rs")):
    for n, line in enumerate(path.read_text().splitlines(), 1):
        found = IGNORE.search(line)
        if not found:
            continue
        if found.group(0) == MARKER:
            journeys += 1
        else:
            errs.append(f"{path.relative_to(ROOT)}:{n}: {found.group(0)}")
if errs:
    errs.append(f"  every #[ignore] reads exactly: {MARKER}")
    errs.append("  a run that costs money is opt-in and user-run, never selected by --ignored")
fail(errs, f"test_tiers ({journeys} tier-2 journeys)")
