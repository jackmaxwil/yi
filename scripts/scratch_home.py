import json
from pathlib import Path


def scratch_config(root: Path, extra=None) -> Path:
    home = root / "home"
    (home / ".yi").mkdir(parents=True)
    config = {"kernel": {"prewarm": False}, **(extra or {})}
    (home / ".yi/config.json").write_text(json.dumps(config, separators=(",", ":")))
    return home
