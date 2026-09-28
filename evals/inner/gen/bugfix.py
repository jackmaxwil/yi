"""Fix failing tests: a small module with planted bugs and a unittest file. Targets the
read-edit-test class: edit refusals, lost tests and done-without-check fire on this shape. The
checker runs its own copy of the tests, so an agent that edits the tests gains nothing. Level 2
has more functions and bugs and grades hidden tests the agent never sees; level 3 shows only one
test per function, so most bugs are found by reading the code, not by running the tests."""
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
# Hidden cases per function: graded from level 2 on, never written into the workspace.
HIDDEN = {
    "clamp": ["self.assertEqual(m.clamp(3, 0, 3), 3)", "self.assertEqual(m.clamp(-1, -5, 5), -1)"],
    "median": ["self.assertEqual(m.median([5]), 5)", "self.assertEqual(m.median([1, 2]), 1.5)"],
    "chunk": ["self.assertEqual(m.chunk([], 2), [])", "self.assertEqual(m.chunk([1, 2, 3, 4], 4), [[1, 2, 3, 4]])"],
    "dedupe": ["self.assertEqual(m.dedupe(['b', 'a', 'b']), ['b', 'a'])"],
    "is_leap": ["self.assertFalse(m.is_leap(2100))", "self.assertTrue(m.is_leap(1996))"],
    "parse_duration": ["self.assertEqual(m.parse_duration('2h'), 7200)",
                       "self.assertEqual(m.parse_duration('1m30s'), 90)"],
    "rot13": ["self.assertEqual(m.rot13('ABC'), 'NOP')", "self.assertEqual(m.rot13(''), '')"],
    "running_max": ["self.assertEqual(m.running_max([]), [])", "self.assertEqual(m.running_max([-2]), [-2])"],
}
# (functions, bugs) per level; level 1 is the first version's shape.
SHAPE = {1: (6, 3), 2: (8, 5), 3: (8, 6)}


def _pick(seed, level=1):
    rng = random.Random(f"bugfix:{seed}" if level == 1 else f"bugfix:{seed}:{level}")
    functions, bugs = SHAPE[level]
    names = rng.sample(sorted(POOL), functions)
    return names, set(rng.sample(names, bugs))


def _module(names, bugged):
    return "\n\n".join(POOL[n][1] if n in bugged else POOL[n][0] for n in names)


def _cases(name, level, graded):
    """The shown cases, plus the hidden ones when grading from level 2 on; level 3 shows one."""
    shown = POOL[name][2][:1] if level == 3 else POOL[name][2]
    return (POOL[name][2] + HIDDEN[name]) if graded and level > 1 else shown


def _tests(names, level=1, graded=False):
    lines = ["import unittest", "", "import toolkit as m", "", "", "class ToolkitTest(unittest.TestCase):"]
    for name in names:
        for index, body in enumerate(_cases(name, level, graded)):
            lines += [f"    def test_{name}_{index}(self):", f"        {body}", ""]
    lines += ["", "if __name__ == '__main__':", "    unittest.main()", ""]
    return "\n".join(lines)


def make(seed, level=1):
    names, bugged = _pick(seed, level)
    prompt = ("The tests in test_toolkit.py fail against toolkit.py. Fix the bugs in toolkit.py so every test "
              "passes. Do not change test_toolkit.py. Run the tests with `python3 -m unittest -v` to check.")
    if level > 1:
        prompt += (" The tests shown are not all the tests toolkit.py will be graded on: every function must be "
                   "correct for all valid inputs, as its name and behavior imply.")
    return {"prompt": prompt, "files": {"toolkit.py": _module(names, bugged), "test_toolkit.py": _tests(names, level)},
            "timeoutSec": 300 if level == 1 else 600}


def check(seed, workspace, level=1):
    """Tests passed out of tests run, against the checker's own copy of the test file (with the
    hidden cases from level 2 on)."""
    names, _ = _pick(seed, level)
    total = sum(len(_cases(n, level, True)) for n in names)
    source = Path(workspace) / "toolkit.py"
    if not source.is_file():
        return 0, total
    with tempfile.TemporaryDirectory() as tmp:
        Path(tmp, "toolkit.py").write_text(source.read_text(errors="replace"))
        Path(tmp, "test_toolkit.py").write_text(_tests(names, level, graded=True))
        try:
            done = subprocess.run([sys.executable, "-m", "unittest", "-v"], cwd=tmp, capture_output=True,
                                  text=True, timeout=60)
        except subprocess.TimeoutExpired:
            return 0, total
    passed = sum(1 for line in done.stderr.splitlines() if line.startswith("test_") and line.endswith("... ok"))
    return passed, total


def solve(seed, workspace, level=1):
    names, _ = _pick(seed, level)
    (Path(workspace) / "toolkit.py").write_text(_module(names, set()))
