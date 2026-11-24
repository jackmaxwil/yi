#!/usr/bin/env python3
"""Lay out target/package/Yi Desktop.app from the dist build and prove it starts headless.

Usage: package.py [--no-build] [--check-only]. The bundle holds the build without the headless
`tooling` feature; the dry run uses a tooling build of the same source kept beside the bundle
(target/package/yi-desktop-tooling), never inside it. Stops at an unsigned bundle: no signing,
no notarising, no upload, no install. Exit 0 when the tooling build, run from inside the bundle
folder, draws a scene.
"""

import argparse
import filecmp
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NAME, EXE, ID = "Yi Desktop", "yi-desktop", "dev.yi.desktop"
BUNDLE = ROOT / "target/package" / f"{NAME}.app"
TOOLING = ROOT / "target/package" / f"{EXE}-tooling"
BUILT = ROOT / "target/dist" / EXE
# Rune's own installer (scripts/mac_app_install.py) stages bundles for macOS 13.
MIN_MACOS = "13.0"

SIGNING = """\
not signed, not notarised, not installed. To ship it, someone with an Apple Developer account needs:
  1. a Developer ID Application certificate in the keychain
  2. codesign --force --options runtime --sign "Developer ID Application: <name> (<team>)" "{app}"
  3. ditto -c -k --keepParent "{app}" Yi-Desktop.zip, then xcrun notarytool submit Yi-Desktop.zip --wait
  4. xcrun stapler staple "{app}"
Until then macOS shows its unidentified-developer warning on first open."""


def version():
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    return manifest["workspace"]["package"]["version"]


def plist():
    return dict(
        CFBundleName=NAME, CFBundleDisplayName=NAME, CFBundleIdentifier=ID,
        CFBundleExecutable=EXE, CFBundlePackageType="APPL",
        CFBundleShortVersionString=version(), CFBundleVersion=version(),
        LSMinimumSystemVersion=MIN_MACOS, NSHighResolutionCapable=True,
        NSPrincipalClass="NSApplication",
        LSApplicationCategoryType="public.app-category.developer-tools",
        # Notifications go through the system's centre under the bundle id; the style is a banner.
        NSUserNotificationAlertStyle="banner",
    )


def lay_out():
    if BUNDLE.exists():
        shutil.rmtree(BUNDLE)
    (BUNDLE / "Contents/MacOS").mkdir(parents=True)
    (BUNDLE / "Contents/Resources").mkdir()
    shutil.copy2(BUILT, BUNDLE / "Contents/MacOS" / EXE)
    data = plist()
    icon = ROOT / "assets/icon.icns"
    if icon.is_file():
        shutil.copy2(icon, BUNDLE / "Contents/Resources/icon.icns")
        data["CFBundleIconFile"] = "icon"
    with (BUNDLE / "Contents/Info.plist").open("wb") as out:
        plistlib.dump(data, out)


def check():
    """The tooling build, run from inside the bundle folder, draws a scene to a picture."""
    macos = BUNDLE / "Contents/MacOS"
    if not (macos / EXE).is_file():
        return f"no executable at {macos / EXE}"
    if not TOOLING.is_file():
        return f"no tooling build at {TOOLING}"
    if filecmp.cmp(macos / EXE, TOOLING, shallow=False):
        return "the bundle holds the tooling build, not the shipping one"
    if any(f.name == TOOLING.name for f in BUNDLE.rglob("*")):
        return "the tooling build is inside the bundle"
    info = plistlib.loads((BUNDLE / "Contents/Info.plist").read_bytes())
    if info.get("CFBundleIdentifier") != ID or info.get("CFBundleExecutable") != EXE:
        return f"Info.plist does not name {ID} and {EXE}: {info}"
    with tempfile.TemporaryDirectory() as home:
        picture = Path(home) / "snap.png"
        run = subprocess.run(
            [str(TOOLING), "--scene", "shell/just-chat", "--scale", "1", "--snap", str(picture)],
            cwd=macos, env={"HOME": home, "PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8"},
            capture_output=True, text=True, timeout=120)
        if run.returncode:
            return f"--snap exited {run.returncode}: {run.stderr[-400:]}"
        if picture.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
            return "--snap wrote no PNG"
    return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--check-only", action="store_true")
    args = parser.parse_args()
    if not args.check_only:
        if not args.no_build:
            build = ["cargo", "build", "--profile", "dist", "-p", EXE]
            subprocess.run([*build, "--features", "tooling"], cwd=ROOT, check=True)
            TOOLING.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(BUILT, TOOLING)
            subprocess.run(build, cwd=ROOT, check=True)
        lay_out()
    problem = check()
    if problem:
        print(f"package: {problem}", file=sys.stderr)
        return 1
    size = sum(f.stat().st_size for f in BUNDLE.rglob("*") if f.is_file())
    print(f"package: {BUNDLE} ({size} bytes) ships the build without tooling")
    print(f"package: {TOOLING} ({TOOLING.stat().st_size} bytes) drew a scene from inside the bundle")
    print(SIGNING.format(app=BUNDLE))
    return 0


if __name__ == "__main__":
    sys.exit(main())
