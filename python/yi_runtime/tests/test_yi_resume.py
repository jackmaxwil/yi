"""F1a: resume reads durable state; an attempt nobody can vouch for stays put (plan section 8.3)."""
from __future__ import annotations

import unittest

import yi.plan
from fake_host import FakeHost
from yi import Plan, Writer, cmd, contract


class Resume(unittest.IsolatedAsyncioTestCase):
    async def test_unknown_external_activity_remains_unresolved(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        accept = contract(cmd("true", critical=True))
        await plan.todo(key="deploy", delegate=Writer(), accept=accept)
        await plan.todo(key="announce", after=["deploy"], delegate=Writer(), accept=accept)
        await plan["deploy"].start()

        # The kernel died: no scheduler, no handles, and the host cannot show the child alive.
        yi.plan._RUNS.clear()
        host.children[f"{plan.id}/deploy"] = "failed"
        host.notices = [f"{plan.id}/deploy needs reconciliation: child cannot be shown alive"]
        written = list(host.journal)

        resumed = await Plan.resume(plan.id)
        self.assertEqual([todo.key for todo in resumed.unresolved], ["deploy"])
        run = await resumed.run(budget=5)
        self.assertEqual(run.outcome, "unresolved")
        self.assertEqual((await run.status())["unresolved"], ["deploy"])
        self.assertEqual(host.journal, written, "an unknown outcome is never retried, failed or restarted")
        self.assertEqual(host.spawns, 1)

    async def test_a_live_child_is_reconnected_and_done_work_is_reused(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        accept = contract(cmd("true", critical=True))
        ran = []

        async def build() -> None:
            ran.append("build")

        await plan.todo(key="build", run=build, accept=accept)
        await plan.todo(key="deploy", after=["build"], delegate=Writer(), accept=accept)
        first = await plan.run(budget=0.2)
        self.assertEqual((first.outcome, ran, host.spawns), ("unresolved", ["build"], 1))

        resumed = await Plan.resume(plan.id)
        await resumed.todo(key="build", run=build, accept=accept)
        self.assertEqual(resumed.unresolved, [])
        host.children[f"{plan.id}/deploy"] = "finished"
        run = await resumed.run(budget=5)
        self.assertEqual((run.outcome, ran, host.spawns), ("verified_success", ["build"], 1))


if __name__ == "__main__":
    unittest.main()
