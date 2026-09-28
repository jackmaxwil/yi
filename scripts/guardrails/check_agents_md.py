#!/usr/bin/env python3
"""AGENTS.md is ruler output that is tracked so a lane carries the rules: it must hold every .ruler
rule verbatim and no rule whose file is gone, or `ruler apply` was not run after an edit."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

agents = (ROOT / "AGENTS.md").read_text() if (ROOT / "AGENTS.md").is_file() else ""
rules = sorted((ROOT / ".ruler").glob("*.md"))
stale = [f".ruler/{rule.name} is not in AGENTS.md; run: ruler apply"
         for rule in rules if rule.read_text().strip() not in agents]
stale += [f"AGENTS.md carries {source}, which no longer exists; run: ruler apply"
          for source in re.findall(r"<!-- Source: (\.ruler/[^ ]+) -->", agents) if not (ROOT / source).is_file()]
fail(stale, "agents_md")
