import os
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def sourced(recipe, home):
    env = {**os.environ, "HOME": str(home)}
    return subprocess.run(
        ["bash", "-c", f". scripts/suite_home.sh {recipe}; echo alive"],
        cwd=ROOT, env=env, capture_output=True, text=True, timeout=60, check=False,
    )


class SuiteHome(unittest.TestCase):
    def test_an_unknown_recipe_leaves_the_sourcing_shell_alive(self):
        out = sourced("no-such-recipe", pathlib.Path("/nonexistent"))
        self.assertEqual(out.stdout.strip(), "alive", out)
        self.assertIn("not a just recipe", out.stderr)

    def test_a_known_recipe_under_the_suite_home_continues_in_place(self):
        out = sourced("test", ROOT / "target" / "yi-home")
        self.assertEqual(out.stdout.strip(), "alive", out)


if __name__ == "__main__":
    unittest.main()
