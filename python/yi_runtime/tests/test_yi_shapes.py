"""F1b: fork_join and scatter schedule under the host's admission and step table (plan sections 8.5, 8.6)."""
from __future__ import annotations

import asyncio
import json
import unittest

import rlm
import yi.plan
from fake_host import FakeHost
from yi import Geometry, Plan, Reader, Writer, cmd, contract, fork_join, review_pod, scatter, schema, shapes
from yi.recipes.review_pod import BRIEFS, declare

GREEN = contract(cmd("true", critical=True))
COMMITTED = contract(schema({"type": "object", "required": ["answer"]}, critical=True))
# What a real lead asks: longer than a label may be, and more than one line of it.
ASKED = "is rotation by age supported anywhere in the flag surface,\nand if so under which name?"
ARCHIVE = {"local://docs/api.md": "usage\nrotate(size) rotates by size\n", "local://docs/cli.md": "flags\n--age DAYS\n"}


def quote(url: str, line: int, text: str) -> dict:
    return {"url": url, "line": line, "text": text}


def product(host: FakeHost, todo) -> str:
    return host.blobs["sha256:" + todo._doc["output"].rsplit("/", 1)[-1]]


async def writers(plan: Plan, host: FakeHost, *keys: str, state: str = "finished") -> None:
    for key in keys:
        await plan.todo(key=key, delegate=Writer(), accept=GREEN)
        host.children[f"{plan.id}/{key}"] = state


async def readers(plan: Plan, host: FakeHost, answers: dict[str, dict | None]) -> None:
    """One reader per archive page; `answers` maps a child's key to what it says, None for a failure."""
    for key, url in zip(("api", "cli"), ARCHIVE):
        await plan.todo(key=key, delegate=Reader(partition=[url]), accept=contract(schema(shapes.ANSWER, critical=True)))
    host.files.update(ARCHIVE)
    for key, said in answers.items():
        child = f"{plan.id}/{key}"
        host.children[child] = "failed" if said is None else "finished"
        host.results[child] = {"text": "", "json": said}


class Shapes(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.levers = (shapes.MAX_RESTARTS, shapes.RESTART_WINDOW, shapes.SCATTER_MAX_ROUNDS)

    def tearDown(self) -> None:
        shapes.MAX_RESTARTS, shapes.RESTART_WINDOW, shapes.SCATTER_MAX_ROUNDS = self.levers
        yi.plan._RUNS.clear()

    async def test_a_fork_join_with_a_shared_write_set_is_refused_with_every_problem_named(self) -> None:
        """Dies with the control: check after the first start, or stop at the first problem."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        await plan.todo(key="man", delegate=Writer(isolation=None), accept=GREEN)
        await plan.todo(key="tests", after=["man"], delegate=Writer(isolation=None), accept=GREEN)

        async def notes() -> None:
            return None

        await plan.todo(key="notes", after=["man"], run=notes)
        with self.assertRaises(Geometry) as refused:
            await plan.run(shape=fork_join, budget=5)
        problems = refused.exception.problems
        self.assertEqual(len(problems), 4, problems)
        for name in ("at least two", "man writes", "tests writes", "notes is inline"):
            self.assertTrue(any(name in problem for problem in problems), (name, problems))
        self.assertEqual((host.starts, host.spawns), ([], 0), "refused before any start")

    async def test_fork_join_retries_a_refused_start_and_never_reorders_the_kernel(self) -> None:
        """Dies with the control: skip past a refused start and `c` is asked for before `b` runs."""
        host = FakeHost()
        host.slots = 1
        plan = await Plan.create("ship it")
        await writers(plan, host, "a", "b", "c")
        run = await plan.run(shape=fork_join, budget=30, detach=True)
        self.assertIs(await plan.run(shape=fork_join, detach=True), run, "a module-level shape attaches to itself")
        await run
        self.assertEqual(run.outcome, "verified_success", await run.status())
        self.assertEqual(host.starts, ["a", "b", "b", "c", "c"], "each refusal is retried, nothing behind it is tried")
        self.assertEqual([args["label"] for op, args in host.journal if op == "start"], ["a", "b", "c"])
        self.assertEqual(run.refusals, {})

    async def test_one_for_one_stops_after_max_within_window(self) -> None:
        """Dies with the control: drop the intensity count and a red todo is retried to the engine's cap."""
        shapes.MAX_RESTARTS = 2
        host = FakeHost()
        plan = await Plan.create("ship it")
        await writers(plan, host, "green", "red")
        host.verdicts["red"] = "fail"
        run = await plan.run(shape=fork_join, budget=30)
        self.assertEqual((run.outcome, host.ops().count("retry"), host.starts.count("red")), ("failed", 2, 3))
        self.assertEqual(plan["green"]._doc["attempt"], 1, "one_for_one restarts the failed todo alone")

        # Outside the window nothing counts, so the engine's own cap is what ends it, and it is obeyed.
        shapes.RESTART_WINDOW, host.retry_cap = 0.0, 4
        await plan["red"].retry()
        run = await plan.run(shape=fork_join, budget=30)
        self.assertEqual((run.outcome, run.refusals["red"].kind), ("failed", "retries_exhausted"))
        self.assertEqual(plan["red"]._doc["attempt"], 5, "no retry past the refusal")

    async def test_the_scheduler_and_the_model_wait_without_stealing_updates(self) -> None:
        """Dies with the control: wait without the run's own cursor and its waits all start at None."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        await writers(plan, host, "a", "b", state="running")
        run = await plan.run(shape=fork_join, budget=30, detach=True)
        await asyncio.sleep(0.05)
        host.children[f"{plan.id}/a"] = "finished"
        await asyncio.sleep(0.05)
        mine = await rlm.wait(timeout=1, cursor=0)
        self.assertIn(f"{plan.id}/a", mine["changed"], "the scheduler saw it first and took nothing")
        host.children[f"{plan.id}/b"] = "finished"
        await run
        self.assertEqual(run.outcome, "verified_success")
        cursors = [cursor for cursor, _ in host.waits if cursor != 0]
        self.assertEqual(cursors[0], None)
        self.assertEqual(cursors[1:], sorted(cursors[1:]), "the scheduler resumes from its own last reply")
        self.assertGreater(cursors[-1], 0)
        self.assertEqual(mine["cursor"], len(host.changes) - 1, "and the model's cursor is its own")

    async def test_scatter_drops_an_unverifiable_quote_before_the_lead_sees_it(self) -> None:
        """Dies with the control: hand the lead the raw answers and the invented quote arrives.

        Invariant: the three ways a quote fails are one seam: a wrong line, a page that will not
        fetch, and a page outside the partition the reader was bound to, which the owner can read.
        """
        host = FakeHost()
        plan = await Plan.create("which module rotates by size?")
        api, cli = ARCHIVE
        await readers(
            plan,
            host,
            {
                "api": {
                    "answer": "rotate()",
                    "quotes": [quote(api, 2, "rotate(size)"), quote(api, 1, "rotate(age)"), quote(cli, 2, "--age DAYS")],
                },
                "cli": {"answer": "the --size flag", "quotes": [quote(cli, 2, "--size BYTES"), quote("local://nope", 1, "x")]},
            },
        )
        seen = []

        async def lead(answers: list, number: int) -> dict:
            seen.append(answers)
            return {"commit": answers[0]["answer"]}

        await plan.todo(key="lead", run=lead, accept=COMMITTED)
        run = await plan.run(shape=scatter, budget=30)
        self.assertEqual(run.outcome, "verified_success", await run.status())
        self.assertEqual([answer["reader"] for answer in seen[0]], ["api"], "an answer with no surviving quote is gone")
        kept = seen[0][0]["quotes"]
        self.assertEqual([item["text"] for item in kept], ["rotate(size)"], "a wrong line and another partition both go")
        self.assertTrue(kept[0]["digest"].startswith("sha256:"), "a kept quote pins what was fetched")
        self.assertEqual(product(host, plan["lead"]), '{"answer":"rotate()","rounds":1}')

    async def test_scatter_ends_at_max_rounds_without_a_commit(self) -> None:
        """Dies with the control: loop while the lead keeps asking and the rounds never end.

        Invariant: the lead writes the question, so it rides the note and never the label, which
        the host caps at eighty characters and refuses a newline in.
        """
        shapes.SCATTER_MAX_ROUNDS = 2
        host = FakeHost()
        plan = await Plan.create("which module rotates by size?")
        api, cli = ARCHIVE
        await readers(plan, host, {"api": {"answer": None, "quotes": [quote(api, 1, "usage")]}, "cli": None})
        for key, cited in (("api-r2", quote(api, 1, "usage")), ("cli-r2", quote(cli, 1, "flags"))):
            host.children[f"{plan.id}/{key}"] = "finished"
            host.results[f"{plan.id}/{key}"] = {"text": "", "json": {"answer": "yes", "quotes": [cited]}}
        rounds = []

        async def lead(answers: list, number: int) -> dict:
            rounds.append((number, [answer["reader"] for answer in answers]))
            return {"ask": ASKED}

        await plan.todo(key="lead", run=lead)
        run = await plan.run(shape=scatter, budget=30)
        self.assertEqual(rounds, [(1, []), (2, ["api-r2", "cli-r2"])], "an abstention and a failed reader are dropped")
        self.assertEqual(plan["lead"]._doc["cause"], "no commit within 2 rounds")
        self.assertEqual((run.outcome, plan["cli"]._doc["state"], host.spawns), ("failed", "abandoned", 4))
        self.assertEqual(plan["api-r2"]._doc["delegation"]["note"], ASKED, "the question rides the note")

    async def test_a_scatter_with_shared_partitions_or_no_lead_is_refused(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ask the archive")
        for key in ("a", "b"):
            await plan.todo(key=key, delegate=Reader(partition=["local://docs"]), accept=GREEN)
        with self.assertRaises(Geometry) as refused:
            await plan.run(shape=scatter, budget=5)
        problems = " | ".join(refused.exception.problems)
        for name in ("a and b share local://docs", "a declares no answer schema", "exactly one lead"):
            self.assertIn(name, problems)
        self.assertEqual(host.starts, [])

    async def pod(self, said: dict[str, dict], verdict: str) -> tuple[FakeHost, Plan, str]:
        """A declared pod over the archive whose readers say `said` and whose command says `verdict`."""
        host = FakeHost()
        plan = await Plan.create("review the rotate change")
        await declare(plan, ["local://docs"], cmd("make -s check", critical=True), arbiter=Writer(isolation=None))
        host.files.update(ARCHIVE)
        for key in BRIEFS:
            child = f"{plan.id}/read-{key}"
            host.children[child] = "finished"
            host.results[child] = {"text": "", "json": said.get(key, {"answer": None, "quotes": []})}
        host.children[f"{plan.id}/arbiter-r2"] = "finished"
        host.verdicts["arbiter-r2"] = verdict
        return host, plan, (await plan.run(shape=review_pod, budget=30)).outcome

    async def test_a_pod_verdict_is_the_arbiters_command_not_a_reader(self) -> None:
        """Dies with the control: let a finding vote and the blocked pod fails or the clean one passes;
        skip the quote seam and the invented finding reaches the arbiter's note."""
        api = next(iter(ARCHIVE))
        blocker = {"answer": "BLOCKER: rotate ignores age", "quotes": [quote(api, 2, "rotate(size)")]}
        invented = {"answer": "BLOCKER: no tests at all", "quotes": [quote(api, 1, "def test_")]}
        host, plan, outcome = await self.pod({"correctness": blocker, "tests": invented}, "pass")
        self.assertEqual(outcome, "verified_success", "two blockers and a green command: the command decides")
        delegation = plan["arbiter-r2"]._doc["delegation"]
        self.assertIn("read-correctness: BLOCKER: rotate ignores age [local://docs/api.md:2]", delegation["note"])
        self.assertIn("read-tests: no backed finding", delegation["note"])
        self.assertNotIn("no tests at all", delegation["note"])
        self.assertEqual(delegation["context"], [plan[f"read-{key}"]._doc["output"] for key in BRIEFS])
        self.assertEqual(plan["arbiter"]._doc["state"], "abandoned", "the declared arbiter ran as its issue")
        self.assertEqual(host.starts.count("arbiter-r2"), 1)

        host, plan, outcome = await self.pod({"scope": {"answer": "x" * 4000, "quotes": [quote(api, 2, "rotate(size)")]}}, "pass")
        note = plan["arbiter-r2"]._doc["delegation"]["note"]
        self.assertLessEqual(len(note.encode("utf-8")), 1024, "the host's InlineNote cap")
        self.assertIn("[cut at 1024 bytes", note, "a finding cut in half never passes for whole")

        host, plan, outcome = await self.pod({}, "fail")
        self.assertEqual(outcome, "failed", "three clean readers and a red command: the command decides")
        dones = [args["label"] for op, args in host.journal if op == "done"]
        self.assertEqual((host.starts.count("arbiter-r2"), dones.count("arbiter-r2")), (1, 0), "verified once, never retried")

    async def test_a_pod_without_a_code_arbiter_or_distinct_briefs_is_refused(self) -> None:
        """Dies with the control: a judged arbiter, or readers sharing one brief, would start."""
        host = FakeHost()
        plan = await Plan.create("review it")
        answer = contract(schema(shapes.ANSWER, critical=True))
        for key in ("one", "two"):
            await plan.todo(key=key, delegate=Reader(partition=["local://docs"], note="read it"), accept=answer)
        await plan.todo(key="arbiter", delegate=Writer(), accept=GREEN)
        items = host.plans[plan.id]["todos"][-1]["contract"]["items"]
        items[0]["critical"] = False
        items.append({"id": "taste", "critical": False, "weight": 1, "decider": {"judge": {}}})
        with self.assertRaises(Geometry) as refused:
            await plan.run(shape=review_pod, budget=5)
        problems = "; ".join(refused.exception.problems)
        for name in ("a brief of its own", "needs a critical cmd or example", "carries a judge item"):
            self.assertIn(name, problems)
        self.assertEqual(host.starts, [])


    async def test_both_shapes_do_useful_work_and_the_overhead_is_counted(self) -> None:
        """The exit measure: host requests through a shape against the same ops sent by hand."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        await writers(plan, host, "a", "b")
        before = sum(host.requests.values())
        self.assertEqual((await plan.run(shape=fork_join, budget=30)).outcome, "verified_success")
        shaped, direct = sum(host.requests.values()) - before, 2 * 2
        self.assertEqual((shaped, direct), (9, 4), "fork_join: a repair, three views and a wait over two starts and two dones")

        host = FakeHost()
        plan = await Plan.create("which module rotates by size?")
        api, cli = ARCHIVE
        said = {"answer": "rotate()", "quotes": [quote(api, 2, "rotate(size)")]}
        await readers(plan, host, {"api": said, "cli": {**said, "quotes": [quote(cli, 2, "--age")]}})

        async def lead(answers: list, number: int) -> dict:
            return {"commit": sorted(answer["reader"] for answer in answers)}

        await plan.todo(key="lead", run=lead, accept=COMMITTED)
        before = sum(host.requests.values())
        self.assertEqual((await plan.run(shape=scatter, budget=30)).outcome, "verified_success")
        self.assertEqual(json.loads(product(host, plan["lead"]))["answer"], ["api", "cli"])
        shaped, direct = sum(host.requests.values()) - before, len(ARCHIVE)
        self.assertEqual((shaped, direct), (18, 2), "scatter: seven ops, four reads, a wait, two results and four fetches")


if __name__ == "__main__":
    unittest.main()
