"""The Yi Desktop bundle that package.py lays out and install.py puts on a Mac."""

import plistlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ID, NAME, EXE = "dev.yi.desktop", "Yi Desktop", "yi-desktop"
BUNDLE = ROOT / "target/package" / f"{NAME}.app"


def read_info(app):
    return plistlib.loads((app / "Contents/Info.plist").read_bytes())
