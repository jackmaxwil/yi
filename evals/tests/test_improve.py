"""The improvement round's offline half: the proposer's corpus, its snapshot, the S0 refusals.
No model, no docker, no network (docs/plans/2026-09-26-self-improvement-evals.md 5.3, 5.5)."""
import contextlib, io, json, os, pathlib, subprocess, sys, tempfile, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "refine"))
import round as rnd  # noqa: E402

SPLIT = {"development": ["alpha", "beta"], "validation": ["gamma"], "final": ["delta"]}


def trial(root, name, task, session_text):
    trial_dir = root / name
    (trial_dir / "agent" / "yi" / "sessions").mkdir(parents=True)
    (trial_dir / "result.json").write_text(json.dumps({"task_name": f"terminal-bench/{task}"}))
    (trial_dir / "agent" / "yi" / "sessions" / "s.jsonl").write_text(session_text)


class Corpus(unittest.TestCase):
    """Leakage is a mount rule: nothing of a validation or final task reaches the proposer."""

    def test_only_development_rows_and_sessions_are_copied(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            store, runs, out = tmp / "trials", tmp / "runs", tmp / "corpus"
            store.mkdir()
            (store / "n1.jsonl").write_text("\n".join(json.dumps(row) for row in (
                {"task": "terminal-bench/alpha", "partialScore": 0.5},
                {"task": "terminal-bench/gamma", "partialScore": 0.9},
                {"task": "terminal-bench/beta", "note": "compare with gamma."},
                {"kind": "verdict", "candidate": "abc", "reason": "pass_lost"},
            )) + "\n")
            trial(runs / "job", "alpha__1", "alpha", '{"kind":"header"}\n')
            trial(runs / "job", "gamma__1", "gamma", '{"kind":"header"}\n')
            trial(runs / "job", "beta__1", "beta", '{"text":"the delta task looked similar"}\n')
            got = rnd.corpus(SPLIT, store, runs, out)
            rows = [json.loads(line) for line in (out / "rows.jsonl").read_text().splitlines()]
            self.assertEqual([row["task"] for row in rows], ["terminal-bench/alpha"])
            self.assertEqual(sorted(p.name for p in (out / "sessions").iterdir()), ["alpha__1"])
            self.assertEqual(got, {"rows": 1, "sessions": 1, "withheld": 5})
            everything = "".join(p.read_text() for p in out.rglob("*") if p.is_file())
            for held in ("gamma", "delta"):
                self.assertNotIn(held, everything)


class Snapshot(unittest.TestCase):
    def test_the_snapshot_has_no_history_no_ledger_and_no_trial_store(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, out = pathlib.Path(tmp) / "repo", pathlib.Path(tmp) / "snap"
            (repo / "docs").mkdir(parents=True)
            (repo / "evals" / "trials").mkdir(parents=True)
            (repo / "docs" / "eval-ledger.md").write_text("| 0055 | gamma 0.5 |\n")
            (repo / "evals" / "trials" / "n1.jsonl").write_text('{"task":"gamma"}\n')
            (repo / "keep.md").write_text("kept\n")
            for argv in (["init", "-q"], ["add", "-A"],
                         ["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "s"]):
                subprocess.run(["git", *argv], cwd=repo, check=True)
            sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True).stdout.strip()
            rnd.snapshot(repo, sha, out)
            self.assertTrue((out / "keep.md").is_file())
            for gone in (".git", "docs/eval-ledger.md", "evals/trials"):
                self.assertFalse((out / gone).exists(), gone)


class S0(unittest.TestCase):
    def patch(self, path, line="+more words\n"):
        return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1,2 @@\n {line}"

    def test_a_patch_outside_the_lever_surface_or_naming_a_held_out_task_is_refused(self):
        self.assertIsNone(rnd.s0(self.patch("crates/runtime/src/prompts/doctrine.md"), {}, SPLIT))
        self.assertIsNone(rnd.s0("", {"loop.cut_stop_at": 4}, SPLIT))
        self.assertEqual(rnd.s0(self.patch("crates/runtime/src/lib.rs"), {}, SPLIT),
                         "path_outside_surface:crates/runtime/src/lib.rs")
        self.assertEqual(rnd.s0(self.patch("crates/tools/src/hashline/prompt.md", "+see gamma\n"), {}, SPLIT),
                         "names_held_out:gamma")
        self.assertEqual(rnd.s0("", {}, SPLIT), "no_change")

    def test_a_candidate_is_its_patch_and_levers_never_its_rationale(self):
        one = rnd.candidate_hash("diff x\n", {"b": 2, "a": 1})
        self.assertEqual(one, rnd.candidate_hash("diff x\n", {"a": 1, "b": 2}))
        self.assertNotEqual(one, rnd.candidate_hash("diff y\n", {"a": 1, "b": 2}))
        with tempfile.TemporaryDirectory() as tmp:
            store = pathlib.Path(tmp)
            (store / "r.jsonl").write_text(json.dumps({"kind": "verdict", "candidate": one, "base": "s1",
                                                        "reason": "pass_lost"}) + "\n")
            self.assertEqual(rnd.rejected(one, "s1", store), "pass_lost")
            self.assertIsNone(rnd.rejected(one, "s2", store), "a new base may try it again")



class Round(unittest.TestCase):
    """The cascade on fakes: S0 and the gate stop a variant before any measurement is paid for,
    a stage that is not better keeps nothing, and the round never spends past --usd."""

    def rows(self, suite, group, cost, graded=0.5):
        tasks = rnd.registry()["suites"][suite]["groups"][group]
        return [[{"task": t, "reward": 0.0, "partialScore": graded, "input": 100, "cacheRead": 0,
                  "output": 0, "costUsd": cost, "wallSec": 10.0} for t in tasks]]

    def cascade(self, found, usd=20.0, build=None, cheaper=0.005):
        self.measured = []

        def measure(stage, suite, group, base_binary, variant_binary, run_id):
            self.measured.append(stage)
            return self.rows(suite, group, 0.01), self.rows(suite, group, cheaper)

        def built(base, patch, work, code):
            return pathlib.Path("/bin/true"), None

        self.git = []
        ran = lambda argv, **kw: self.git.append(argv) or subprocess.CompletedProcess(argv, 0, "", "")
        with tempfile.TemporaryDirectory() as store, mock.patch.object(rnd.subprocess, "run", ran):
            self.work = pathlib.Path(store) / f"work-{rnd.candidate_hash(found[0][1], found[0][2])}"
            return rnd.run_round(found, "base0", "/bin/true", usd, SPLIT, store=self.store or store,
                                 build_fn=build or built, measure_fn=measure, out=store)

    store = None

    def patch(self, path):
        return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n"

    def test_a_cheaper_variant_passes_every_stage_and_is_kept(self):
        got = self.cascade([("v", self.patch("crates/runtime/src/prompts/doctrine.md"), {}, "t")])
        row = got["variants"][0]
        self.assertEqual([s["stage"] for s in row["stages"]], ["screen", "validate", "transfer"])
        self.assertTrue(row["kept"].startswith("refine/"), row)
        self.assertIn(["git", "worktree", "remove", "--force", str(self.work)], self.git, "a dist build per variant")

    def test_a_variant_no_better_than_its_base_keeps_nothing(self):
        got = self.cascade([("v", self.patch("crates/runtime/src/prompts/doctrine.md"), {}, "t")], cheaper=0.01)
        row = got["variants"][0]
        self.assertNotIn("kept", row)
        self.assertEqual(self.measured, ["screen", "validate"], "validation is inconclusive and stops it")

    def test_a_red_gate_and_a_types_edit_are_never_measured(self):
        red = lambda base, patch, work, code: (None, "gate_red")
        got = self.cascade([("code", self.patch("crates/loop/src/batch.rs"), {}, "t")], build=red)
        self.assertEqual(got["variants"][0]["refused"], "gate_red")
        self.assertTrue(got["variants"][0]["code"])
        got = self.cascade([("types", self.patch("crates/types/src/event.rs"), {}, "t")])
        self.assertEqual(got["variants"][0]["refused"], "path_outside_surface:crates/types/src/event.rs")
        self.assertEqual(self.measured, [])

    def test_a_refused_variant_is_stored_so_the_next_round_never_rebuilds_it(self):
        built = []
        red = lambda base, patch, work, code: built.append(work) or (None, "gate_red")
        found = [("code", self.patch("crates/loop/src/batch.rs"), {}, "t"),
                 ("types", self.patch("crates/types/src/event.rs"), {}, "t")]
        with tempfile.TemporaryDirectory() as self.store:
            self.cascade(found, build=red)
            got = self.cascade(found, build=red)
        self.assertEqual(len(built), 1, "the red gate ran again on the same base")
        self.assertEqual([row["refused"] for row in got["variants"]],
                         ["already_rejected:gate_red", "already_rejected:path_outside_surface:crates/types/src/event.rs"])

    def test_a_variants_build_and_gate_see_a_scratch_home_and_no_key(self):
        seen = []
        ran = lambda argv, **kw: seen.append((argv, kw.get("env"))) or subprocess.CompletedProcess(argv, 0, "", "")
        owner = {"HOME": "/owner", "CARGO_HOME": "/owner/.cargo", "OPENROUTER_API_KEY": "sk-or-planted"}
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, owner), \
                mock.patch.object(rnd.subprocess, "run", ran):
            work = pathlib.Path(tmp) / "work-v"
            rnd.build("base0", self.patch("crates/loop/src/batch.rs"), work, True)
        steps = [(argv, env) for argv, env in seen if env is not None]
        self.assertEqual([argv[0] for argv, _ in steps], ["cargo", "just"])
        for _, env in steps:
            self.assertEqual(env["HOME"], str(pathlib.Path(tmp) / "home-work-v"))
            self.assertEqual(env["CARGO_HOME"], "/owner/.cargo")
            self.assertNotIn("sk-or-planted", json.dumps(env))

    def test_a_stage_the_budget_cannot_cover_is_not_run(self):
        got = self.cascade([("v", self.patch("crates/runtime/src/prompts/doctrine.md"), {}, "t")], usd=0.5)
        self.assertEqual(self.measured, [])
        self.assertTrue(got["variants"][0]["stopped"].startswith("screen:"), got)


class Keep(unittest.TestCase):
    def test_a_kept_branch_carries_its_patch_and_leaves_no_worktree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, git = pathlib.Path(tmp), lambda *argv: subprocess.run(
                ["git", *argv], cwd=tmp, check=True, capture_output=True, text=True).stdout
            (repo / "a.txt").write_text("a\n")
            for argv in (["init", "-q"], ["config", "user.name", "t"], ["config", "user.email", "t@example.invalid"],
                         ["add", "-A"], ["commit", "-qm", "s"]):
                git(*argv)
            patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n"
            row = {"variant": rnd.candidate_hash(patch, {}), "kept": "refine/v", "target": "t"}
            with contextlib.redirect_stdout(io.StringIO()):
                rnd.keep({"variants": [row]}, git("rev-parse", "HEAD").strip(), [("v", patch, {}, "t")], repo=repo)
            self.assertEqual(git("show", "refine/v:a.txt"), "b\n")
            self.assertEqual(git("worktree", "list", "--porcelain").count("worktree "), 1)


if __name__ == "__main__":
    unittest.main()
