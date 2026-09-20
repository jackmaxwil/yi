"""F1a: create, the source record, the scheduler and its lease (plan sections 8.2 and 8.3)."""
from __future__ import annotations

import asyncio
import unittest

import yi.plan
from fake_host import FakeHost
from yi import Plan, PlanError, RunActive, Writer, cmd, contract, in_order, schema

REPORT = {"type": "object", "required": ["passed"]}


class Plans(unittest.IsolatedAsyncioTestCase):
    def tearDown(self) -> None:
        yi.plan._CELL = None

    async def test_a_retried_create_request_is_the_same_plan(self) -> None:
        host = FakeHost()
        first = await Plan.create("ship logrotate-lite", request_id="create-01")
        again = await Plan.create("ship logrotate-lite", request_id="create-01")
        self.assertEqual((again.id, len(host.plans), host.ops()), (first.id, 1, ["init"]))
        with self.assertRaises(PlanError) as reused:
            await Plan.create("ship something else", request_id="create-01")
        self.assertEqual(reused.exception.kind, "request_id_reused")
        with self.assertRaises(PlanError) as guessed:
            await Plan.create("ship logrotate-lite")
        self.assertEqual(guessed.exception.kind, "plan_exists", "create never adopts a plan by its text")

    async def test_a_cell_is_recorded_once_before_its_first_effect(self) -> None:
        host = FakeHost()

        class Cell:
            raw_cell = "plan = await Plan.create('ship it')\nawait plan.todo(key='a')\n"

        yi.plan._on_cell(Cell)
        plan = await Plan.create("ship it")
        self.assertEqual(host.ops(), ["init", "program"], "a cell that only creates is recorded too")
        await plan.todo(key="a")
        await plan.todo(key="a")
        self.assertEqual(host.ops(), ["init", "program", "append"])
        digest = host.journal[1][1]["source_ref"]["digest"]
        self.assertEqual(host.blobs[digest], Cell.raw_cell)

        yi.plan._on_cell(Cell)
        await (await Plan.attach(plan.id)).refresh()
        self.assertEqual(host.ops().count("program"), 1, "a cell that only reads records nothing")
        await plan["a"].start()
        self.assertEqual(host.ops()[-2:], ["program", "start"], "the record precedes the effect")

    async def test_the_example_runs_end_to_end(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship logrotate-lite with a packaged tarball", request_id="create-01")

        async def freeze_surface() -> dict:
            return {"passed": 3}

        freeze = await plan.todo(
            key="freeze",
            label="freeze the CLI surface",
            run=freeze_surface,
            accept=contract(schema(REPORT, critical=True)),
        )
        tests = await plan.todo(
            key="tests",
            label="write the test suite",
            after=[freeze],
            delegate=Writer(isolation="worktree", deny_write=["docs/"]),
            accept=contract(cmd("pytest -q tests/", critical=True), schema(REPORT, critical=True)),
        )
        child = f"{plan.id}/tests"
        host.children[child] = "finished"
        host.results[child] = {"text": "{}", "json": {"passed": 12}}
        run = await plan.run(budget="2h")
        self.assertEqual(run.outcome, "verified_success", await run.status())
        self.assertEqual(host.spawns, 1)
        for todo in (freeze, tests):
            self.assertTrue(todo._doc["output"].startswith(f"plan://{plan.id}/artifacts/"), todo._doc)
        self.assertIn('{"passed":12}', host.blobs.values())

    async def test_a_refused_done_fails_the_attempt_and_a_stuck_child_is_waited_on(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="tests", delegate=Writer(), accept=contract(cmd("false", critical=True)))
        host.verdicts[todo.label] = "fail"
        host.children[f"{plan.id}/tests"] = "stuck"
        run = await plan.run(budget=30, detach=True)
        await asyncio.sleep(0.1)
        self.assertEqual((await run.status())["states"], {"tests": "running"}, "stuck is not failed")
        host.children[f"{plan.id}/tests"] = "finished"
        await run
        self.assertEqual((run.outcome, todo._doc["state"]), ("failed", "failed"))
        self.assertIn("done refused", todo._doc["cause"])

    async def test_duplicate_run_calls_attach_or_refuse(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        await plan.todo(key="tests", delegate=Writer(), accept=contract(cmd("true", critical=True)))
        first = await plan.run(budget=30, detach=True)
        second = await plan.run(budget=30, detach=True)
        self.assertIs(second, first, "the same shape attaches to the running scheduler")

        async def other(plan, run) -> None:
            await in_order(plan, run)

        with self.assertRaises(RunActive) as refused:
            await plan.run(shape=other, detach=True)
        self.assertEqual(refused.exception.run_id, first.id)

        host.children[f"{plan.id}/tests"] = "finished"
        await first
        self.assertEqual((first.outcome, host.spawns), ("verified_success", 1))
        third = await plan.run(shape=other, budget=5)
        self.assertIsNot(third, first, "a finished run holds no lease")

    async def test_stop_cancels_what_is_active_and_reports_what_remains(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        started = asyncio.Event()

        async def forever() -> None:
            started.set()
            await asyncio.sleep(3600)

        await plan.todo(key="inline", run=forever)
        await plan.todo(key="child", delegate=Writer(), accept=contract(cmd("true", critical=True)))
        await plan.todo(key="later", after=["child"], delegate=Writer(), accept=contract(cmd("true", critical=True)))
        run = await plan.run(detach=True)
        await started.wait()
        status = await run.stop(scope="cancel_active")
        self.assertEqual(status["outcome"], "cancelled")
        self.assertEqual(status["states"], {"inline": "failed", "child": "failed", "later": "pending"})
        self.assertEqual(host.children[f"{plan.id}/child"], "failed", "the child was interrupted")


if __name__ == "__main__":
    unittest.main()
