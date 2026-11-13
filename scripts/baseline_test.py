import contextlib
import io
import pathlib
import types
import unittest

import baseline
import forge_pr


class Baseline(unittest.TestCase):
    def test_the_fork_point_runs_in_a_throwaway_worktree_and_head_in_the_tree(self):
        calls = []
        exits = iter([3, 0])

        def runner(command, cwd, env, check):
            calls.append((pathlib.Path(cwd), env.get("CARGO_TARGET_DIR"), pathlib.Path(cwd).exists()))
            return types.SimpleNamespace(returncode=next(exits))

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = baseline.main("test(x)", base="HEAD", runner=runner)
        (old_dir, old_target, old_existed), (new_dir, new_target, _) = calls
        self.assertTrue(old_existed)
        self.assertNotEqual(old_dir, forge_pr.ROOT)
        self.assertEqual(pathlib.Path(old_target).parent, old_dir.parent)
        self.assertFalse(old_dir.parent.exists(), "the worktree and its target dir are gone after the run")
        self.assertEqual(new_dir, forge_pr.ROOT)
        self.assertIsNone(new_target)
        self.assertEqual(code, 0)
        self.assertIn("baseline exit 3, head exit 0", out.getvalue())


if __name__ == "__main__":
    unittest.main()
