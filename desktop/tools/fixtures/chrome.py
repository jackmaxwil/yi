"""Drives headless Chrome over a copy of the built mockup page and returns what probe.js wrote."""
import html
import json
import re
import subprocess
import tempfile
from pathlib import Path

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PROBE = (Path(__file__).parent / "probe.js").read_text()


class ChromeError(Exception):
    pass


def run_frame(page_text, frame, spec, query=""):
    """Load frame `frame` of the page (settled at t=20) with `spec` injected; return the probe's JSON."""
    doc = page_text + "<script>window.__SPEC=" + json.dumps(spec).replace("</", "<\\/") + "</script><script>" + PROBE + "</script>"
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "page.html"
        path.write_text(doc)
        done = subprocess.run(
            [CHROME, "--headless", "--disable-gpu", "--virtual-time-budget=7000", "--dump-dom", f"file://{path}?solo={frame}&t=20{'&' + query if query else ''}"],
            capture_output=True, text=True, timeout=120)
    found = re.search(r'<pre id="probe-out">(.*?)</pre>', done.stdout, re.S)
    if not found:
        raise ChromeError(f"frame {frame}: no probe output (exit {done.returncode}): {done.stderr[-300:]}")
    out = json.loads(html.unescape(found.group(1)))
    if "error" in out:
        raise ChromeError(f"frame {frame}: {out['error']}")
    return out
