"""The improvement round's offline half: the proposer's corpus, its snapshot, the S0 refusals.
No model, no docker, no network (docs/plans/2026-09-26-self-improvement-evals.md 5.3, 5.5)."""
import json, pathlib, subprocess, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "improve"))
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


if __name__ == "__main__":
    unittest.main()
