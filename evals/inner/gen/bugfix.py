"""Fix failing tests: a small module with planted bugs and a unittest file. Targets the
read-edit-test class: edit refusals, lost tests and done-without-check fire on this shape. The
checker runs its own copy of the tests, so an agent that edits the tests gains nothing."""
import random
import subprocess
import sys
import tempfile
from pathlib import Path

# name: (correct source, buggy source, test body). Each test body is one unittest method's lines.
POOL = {
    "clamp": ("def clamp(x, lo, hi):\n    return max(lo, min(x, hi))\n",
              "def clamp(x, lo, hi):\n    return min(lo, max(x, hi))\n",
              ["self.assertEqual(m.clamp(5, 0, 3), 3)", "self.assertEqual(m.clamp(-2, 0, 3), 0)",
               "self.assertEqual(m.clamp(2, 0, 3), 2)"]),
    "median": ("def median(xs):\n    s = sorted(xs)\n    n = len(s)\n    mid = n // 2\n"
               "    return s[mid] if n % 2 else (s[mid - 1] + s[mid]) / 2\n",
               "def median(xs):\n    s = sorted(xs)\n    n = len(s)\n    mid = n // 2\n"
               "    return s[mid] if n % 2 else (s[mid] + s[mid + 1]) / 2\n",
               ["self.assertEqual(m.median([3, 1, 2]), 2)", "self.assertEqual(m.median([4, 1, 3, 2]), 2.5)"]),
    "chunk": ("def chunk(xs, n):\n    return [xs[i:i + n] for i in range(0, len(xs), n)]\n",
              "def chunk(xs, n):\n    return [xs[i:i + n] for i in range(0, len(xs) - 1, n)]\n",
              ["self.assertEqual(m.chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]])",
               "self.assertEqual(m.chunk([1], 3), [[1]])"]),
    "dedupe": ("def dedupe(xs):\n    seen, out = set(), []\n    for x in xs:\n        if x not in seen:\n"
               "            seen.add(x)\n            out.append(x)\n    return out\n",
               "def dedupe(xs):\n    return sorted(set(xs))\n",
               ["self.assertEqual(m.dedupe([3, 1, 3, 2, 1]), [3, 1, 2])", "self.assertEqual(m.dedupe([]), [])"]),
    "is_leap": ("def is_leap(y):\n    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)\n",
                "def is_leap(y):\n    return y % 4 == 0 and y % 100 != 0\n",
                ["self.assertTrue(m.is_leap(2024))", "self.assertFalse(m.is_leap(1900))",
                 "self.assertTrue(m.is_leap(2000))"]),
    "parse_duration": ("def parse_duration(text):\n    units = {'h': 3600, 'm': 60, 's': 1}\n    total, number = 0, ''\n"
                       "    for ch in text:\n        if ch.isdigit():\n            number += ch\n        else:\n"
                       "            total += int(number) * units[ch]\n            number = ''\n    return total\n",
                       "def parse_duration(text):\n    units = {'h': 3600, 'm': 60, 's': 1}\n    total, number = 0, ''\n"
                       "    for ch in text:\n        if ch.isdigit():\n            number += ch\n        else:\n"
                       "            total += int(number) * units[ch]\n    return total\n",
                       ["self.assertEqual(m.parse_duration('1h2m3s'), 3723)",
                        "self.assertEqual(m.parse_duration('45s'), 45)"]),
    "rot13": ("def rot13(s):\n    out = []\n    for ch in s:\n        if 'a' <= ch <= 'z':\n"
              "            out.append(chr((ord(ch) - 97 + 13) % 26 + 97))\n        elif 'A' <= ch <= 'Z':\n"
              "            out.append(chr((ord(ch) - 65 + 13) % 26 + 65))\n        else:\n            out.append(ch)\n"
              "    return ''.join(out)\n",
              "def rot13(s):\n    out = []\n    for ch in s:\n        if 'a' <= ch <= 'z':\n"
              "            out.append(chr((ord(ch) - 97 + 13) % 26 + 97))\n        else:\n            out.append(ch)\n"
              "    return ''.join(out)\n",
              ["self.assertEqual(m.rot13('Hello, World!'), 'Uryyb, Jbeyq!')", "self.assertEqual(m.rot13('abc'), 'nop')"]),
    "running_max": ("def running_max(xs):\n    out, best = [], None\n    for x in xs:\n"
                    "        best = x if best is None or x > best else best\n        out.append(best)\n    return out\n",
                    "def running_max(xs):\n    out, best = [], 0\n    for x in xs:\n"
                    "        best = x if x > best else best\n        out.append(best)\n    return out\n",
                    ["self.assertEqual(m.running_max([2, 5, 1, 7]), [2, 5, 5, 7])",
                     "self.assertEqual(m.running_max([-3, -5, -1]), [-3, -3, -1])"]),
}
FUNCTIONS, BUGS = 6, 3


def _pick(seed):
    rng = random.Random(f"bugfix:{seed}")
    names = rng.sample(sorted(POOL), FUNCTIONS)
    return names, set(rng.sample(names, BUGS))


def _module(names, bugged):
    return "\n\n".join(POOL[n][1] if n in bugged else POOL[n][0] for n in names)


def _tests(names):
    lines = ["import unittest", "", "import toolkit as m", "", "", "class ToolkitTest(unittest.TestCase):"]
    for name in names:
        for index, body in enumerate(POOL[name][2]):
            lines += [f"    def test_{name}_{index}(self):", f"        {body}", ""]
    lines += ["", "if __name__ == '__main__':", "    unittest.main()", ""]
    return "\n".join(lines)


def make(seed):
    names, bugged = _pick(seed)
    prompt = ("The tests in test_toolkit.py fail against toolkit.py. Fix the bugs in toolkit.py so every test "
              "passes. Do not change test_toolkit.py. Run the tests with `python3 -m unittest -v` to check.")
    return {"prompt": prompt, "files": {"toolkit.py": _module(names, bugged), "test_toolkit.py": _tests(names)},
            "timeoutSec": 300}


def check(seed, workspace):
    """Tests passed out of tests run, against the checker's own copy of the test file."""
    names, _ = _pick(seed)
    total = sum(len(POOL[n][2]) for n in names)
    source = Path(workspace) / "toolkit.py"
    if not source.is_file():
        return 0, total
    with tempfile.TemporaryDirectory() as tmp:
        Path(tmp, "toolkit.py").write_text(source.read_text(errors="replace"))
        Path(tmp, "test_toolkit.py").write_text(_tests(names))
        try:
            done = subprocess.run([sys.executable, "-m", "unittest", "-v"], cwd=tmp, capture_output=True,
                                  text=True, timeout=60)
        except subprocess.TimeoutExpired:
            return 0, total
    passed = sum(1 for line in done.stderr.splitlines() if line.startswith("test_") and line.endswith("... ok"))
    return passed, total


def solve(seed, workspace):
    names, _ = _pick(seed)
    (Path(workspace) / "toolkit.py").write_text(_module(names, set()))
