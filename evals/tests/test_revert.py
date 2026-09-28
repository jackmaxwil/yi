"""Real past fixes, redone: a commit that changed the library and its tests, with the library change
undone, graded by the commit's own tests. A synthetic git repo stands in for the cloned ones so this
runs offline in CI."""
import pathlib, shutil, subprocess, sys, tempfile, textwrap, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "inner"))
import gen  # noqa: E402
from gen import mutate, revert  # noqa: E402

CORE = "def slug(s):\n    return s.lower()\n\n\ndef count(xs):\n    return len(xs)\n"
TESTS = textwrap.dedent('''\
    import unittest
    from lib import core


    class Core(unittest.TestCase):
        def test_lower(self):
            self.assertEqual(core.slug("AB"), "ab")

        def test_count(self):
            self.assertEqual(core.count([1, 2]), 2)
    ''')
STRIP = ("Trim slugs\n\nslug() now strips surrounding whitespace.",
         CORE.replace("s.lower()", "s.strip().lower()"),
         TESTS + '\n    def test_strip(self):\n        self.assertEqual(core.slug(" A "), "a")\n')
JOIN = ("Add join\n\njoin() glues words with dashes.",
        STRIP[1] + '\n\ndef join(xs):\n    return "-".join(xs)\n',
        STRIP[2] + '\n    def test_join(self):\n        self.assertEqual(core.join(["a", "b"]), "a-b")\n')


class Revert(unittest.TestCase):
    def setUp(self):
        self.root = pathlib.Path(tempfile.mkdtemp())
        self.repo = repo = self.root / "toy"
        (repo / "lib").mkdir(parents=True)
        (repo / "tests").mkdir()
        (repo / "lib" / "__init__.py").write_text("")
        (repo / "tests" / "__init__.py").write_text("")
        self.commit("First", CORE, TESTS, init=True)
        for message, core, tests in (STRIP, JOIN):
            self.commit(message, core, tests)
        self.saved = dict(mutate.REPOS), revert.CACHE
        mutate.REPOS.clear()
        mutate.REPOS["toy"] = {"path": repo, "src": "lib", "tests": "tests", "commit": None}
        revert.CACHE = self.root / "cache"
        self.saved_sizes = revert.SIZES[3]

    def commit(self, message, core, tests, init=False):
        (self.repo / "lib" / "core.py").write_text(core)
        (self.repo / "tests" / "test_core.py").write_text(tests)
        git = ["git", "-C", str(self.repo)]
        if init:
            subprocess.run(git + ["init", "-q"], check=True)
        subprocess.run(git + ["add", "-A"], check=True)
        subprocess.run(git + ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false",
                              "commit", "-q", "-m", message], check=True)

    def tearDown(self):
        mutate.REPOS.clear()
        mutate.REPOS.update(self.saved[0])
        revert.CACHE = self.saved[1]
        shutil.rmtree(self.root, ignore_errors=True)

    def test_a_seed_is_a_past_change_undone_and_graded_by_its_tests(self):
        messages = set()
        for seed in (0, 1):
            with self.subTest(seed=seed):
                task = revert.make(seed, 1)
                self.assertEqual(task, revert.make(seed, 1), "a seed is one task, every time")
                messages.add(task["prompt"].split("\n\n")[1])
                workspace = gen.materialize(task, self.root / f"w{seed}")
                self.assertFalse((workspace / ".git").exists(), "no history to read the change back out of")
                passed, total = revert.check(seed, workspace, 1)
                self.assertEqual(total, 2, "one test the commit turned green, plus the clean point")
                self.assertLess(passed, total, "an untouched workspace is red")
                revert.solve(seed, workspace, 1)
                self.assertEqual(revert.check(seed, workspace, 1), (total, total))
                shutil.rmtree(workspace / "tests")
                self.assertEqual(revert.check(seed, workspace, 1), (total, total), "the checker brings its tests")
        self.assertEqual(messages, {"Trim slugs", "Add join"}, "each commit's message is its request")

    def test_levels_2_and_3_hide_the_commits_tests(self):
        with self.assertRaises(RuntimeError, msg="level 3 wants a bigger change than the toy has"):
            revert.make(0, 3)
        revert.SIZES = {**revert.SIZES, 3: revert.SIZES[2]}
        self.addCleanup(setattr, revert, "SIZES", {**revert.SIZES, 3: self.saved_sizes})
        for level in (2, 3):
            with self.subTest(level=level):
                task = revert.make(0, level)
                plan = revert._plan(0, level)
                workspace = gen.materialize(task, self.root / f"h{level}")
                visible = mutate._suite(workspace, mutate.REPOS["toy"])
                for test in plan["f2p"]:
                    self.assertNotIn(test, visible, "the commit's new test is hidden")
                    self.assertIn(f"{test}: {plan['notes'][test]}", task["prompt"], "its failure line is shown instead")
                    self.assertTrue(plan["notes"][test].startswith("AttributeError"))
                self.assertLess(revert.check(0, workspace, level)[0], len(plan["f2p"]) + 1)
                revert.solve(0, workspace, level)
                self.assertEqual(revert.check(0, workspace, level), (len(plan["f2p"]) + 1,) * 2)


if __name__ == "__main__":
    unittest.main()
