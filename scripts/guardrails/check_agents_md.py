#!/usr/bin/env python3
"""AGENTS.md is ruler output that is tracked so a lane carries the rules: every .ruler rule must
appear in it verbatim, or `ruler apply --agents pi --skills false` was not run after an edit."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

agents = (ROOT / "AGENTS.md").read_text() if (ROOT / "AGENTS.md").is_file() else ""
stale = [f".ruler/{rule.name} is not in AGENTS.md; run: ruler apply --agents pi --skills false"
         for rule in sorted((ROOT / ".ruler").glob("*.md")) if rule.read_text().strip() not in agents]
fail(stale, "agents_md")
