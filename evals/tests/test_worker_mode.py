"""EXP-B1e's denial counter and its pre-registered verdict, on two worker transcripts a real
offline run recorded (`evals/fixtures/worker_mode/`): the first sends the same refused write
again, the second follows the refusal with a different reversible form."""
import json
import sys
import tempfile
import unittest
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

import worker_mode  # noqa: E402

FIXTURES = EVALS / "fixtures" / "worker_mode"


def messages(name):
    return worker_mode.entries(FIXTURES / f"{name}.jsonl")


def lane(root, spec_id, name):
    """One run's directory as the harness writes it: a worker transcript under sessions/, a repo."""
    run = Path(root) / spec_id
    session = run / "sessions" / "ws" / "parent" / "children" / spec_id
    session.mkdir(parents=True)
    (session / "child.jsonl").write_text((FIXTURES / f"{name}.jsonl").read_text())
    (run / "repo").mkdir()
    return run


class Counter(unittest.TestCase):
    def test_a_refused_command_sent_again_is_one_retry(self):
        # Failure a consumer sees: the counter reported 0 retries for a worker that resent the
        # same refused write, so the arm's whole metric reads as a worker that never looped.
        self.assertEqual(worker_mode.retries(messages("retried")), ["write ../tally.txt"])
        self.assertEqual(len(worker_mode.denials(messages("retried"))), 2)

    def test_a_reversible_form_after_a_refusal_is_not_a_retry(self):
        # Failure a consumer sees: comparing by tool name alone scores the worker that answered a
        # refused `write ../tally.txt` with `write tally.txt` as a retry, so the arm that changed
        # approach is charged with looping and the withdrawal fires on a worker that behaved.
        self.assertEqual(worker_mode.denials(messages("reversible")),
                         ["write ../tally.txt", "write tally.txt"])
        self.assertEqual(worker_mode.retries(messages("reversible")), [])

    def test_a_run_is_scored_by_its_worker_transcript_and_its_report(self):
        # Failure a consumer sees: a run with no worker transcript scored 0 retries and passed,
        # so an arm whose children never started looked like the cleanest arm of the experiment.
        with tempfile.TemporaryDirectory() as tmp:
            run = lane(tmp, "write-outside-with-mode", "retried")
            self.assertEqual(worker_mode.score(run, "write-outside-with-mode"),
                             {"task": "write-outside", "arm": "with-mode", "workers": 1,
                              "denied": 2, "retries": 1, "failed": True})
            (run / "repo" / worker_mode.REPORT).write_text("could not write outside\n")
            self.assertFalse(worker_mode.score(run, "write-outside-with-mode")["failed"])

    def test_a_family_verdict_withdraws_on_a_rise_and_waits_for_the_other_family(self):
        # Failure a consumer sees: the verdict kept the drop while the fragment-free arm retried
        # more, or reported KEEP while one family had not run at all.
        table = {"with-mode": {"retries": 1, "failed": 2, "tasks": 10},
                 "without-mode": {"retries": 3, "failed": 2, "tasks": 10}}
        self.assertTrue(worker_mode.verdict({"kimi": table}, True).startswith("WITHDRAW"))
        held = {"with-mode": {"retries": 2, "failed": 1, "tasks": 10},
                "without-mode": {"retries": 2, "failed": 1, "tasks": 10}}
        self.assertTrue(worker_mode.verdict({"kimi": held}, True).startswith("KEEP"))
        self.assertTrue(worker_mode.verdict({"kimi": held}, False).startswith("UNRESOLVED"))

    def test_the_families_arm_specs_name_their_binary_and_run_under_auto(self):
        # Failure a consumer sees: both arms ran the same head binary, so the experiment compared
        # the drop against itself and could never widen a figure.
        import os
        from unittest import mock
        with mock.patch.dict(os.environ, {worker_mode.ARM_BINARY_ENV: "/tmp/yi-with-mode"}):
            specs = worker_mode.run_specs()
        self.assertEqual(len(specs), 2 * len(worker_mode.TASKS))
        arms = {worker_mode.arm_of(spec["id"]) for spec in specs}
        self.assertEqual(arms, set(worker_mode.ARMS))
        for spec in specs:
            self.assertIn("--auto", spec["args"])
            self.assertEqual(spec.get("binary"),
                             "/tmp/yi-with-mode" if spec["id"].endswith("with-mode") else None)
        with mock.patch.dict(os.environ, {worker_mode.ARM_BINARY_ENV: ""}):
            alone = worker_mode.run_specs()
        self.assertEqual({worker_mode.arm_of(spec["id"]) for spec in alone}, {"without-mode"})


if __name__ == "__main__":
    unittest.main()
