#!/usr/bin/env python3
"""Refresh reasoning-effort metadata in crates/ai/data/openrouter.json.

Pi's catalog generator drops OpenRouter's per-model reasoning contract, so a
plain catalog refresh reintroduces HTTP 400 "Reasoning is mandatory for this
endpoint and cannot be disabled". Run this after every refresh.
"""

import collections
import json
import pathlib
import urllib.request

MODELS_URL = "https://openrouter.ai/api/v1/models"
CATALOG = pathlib.Path(__file__).resolve().parent.parent / "crates/ai/data/openrouter.json"
RANK = {"none": 0, "minimal": 1, "low": 2, "medium": 3, "high": 4, "xhigh": 5, "max": 6}
LEVELS = ["minimal", "low", "medium", "high", "xhigh", "max"]


def nearest(level, supported):
    return min(supported, key=lambda effort: (abs(RANK[effort] - RANK[level]), RANK[effort]))


def main():
    with urllib.request.urlopen(MODELS_URL, timeout=30) as response:
        live = {model["id"]: model for model in json.load(response)["data"]}
    baked = json.loads(CATALOG.read_text(), object_pairs_hook=collections.OrderedDict)

    changed = 0
    for models in baked.values():
        for model_id, entry in models.items():
            reasoning = (live.get(model_id) or {}).get("reasoning") or {}
            if not entry.get("reasoning") or not reasoning:
                continue
            supported = [e for e in (reasoning.get("supported_efforts") or []) if e in RANK and e != "none"]
            level_map = collections.OrderedDict(entry.get("thinkingLevelMap") or {})
            before = json.dumps(level_map)
            for level in LEVELS if supported else []:
                if level in supported:
                    level_map.pop(level, None)
                else:
                    level_map[level] = nearest(level, supported)
            if reasoning.get("mandatory"):
                level_map["off"] = None
            else:
                level_map.pop("off", None)
            if json.dumps(level_map) == before:
                continue
            changed += 1
            entry.pop("thinkingLevelMap", None)
            if level_map:
                entry["thinkingLevelMap"] = level_map

    CATALOG.write_text(json.dumps(baked, separators=(",", ":")) + "\n")
    print(f"models updated: {changed}")


if __name__ == "__main__":
    main()
