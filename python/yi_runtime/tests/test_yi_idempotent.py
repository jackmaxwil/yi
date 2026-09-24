"""F1a: a todo is declared once by key; a changed redeclaration is refused (plan section 8.2)."""
from __future__ import annotations

import unittest

from fake_host import FakeHost
from yi import Plan, SpecDrift, Writer, cmd, contract


class Idempotent(unittest.IsolatedAsyncioTestCase):
    async def declare(self, plan: Plan, command: str = "pytest -q tests/"):
        freeze = await plan.todo(key="freeze", label="freeze the CLI surface")
        return await plan.todo(
            key="tests",
            label="write the test suite",
            after=[freeze],
            delegate=Writer(accept=contract(cmd(command, critical=True)), deny_write=["docs/"]),
        )

    async def test_a_rerun_cell_writes_nothing_and_a_changed_spec_is_refused(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship logrotate-lite")
        first = await self.declare(plan)
        written = list(host.journal)
        again = await self.declare(plan)
        self.assertEqual((again.key, again.label), (first.key, first.label))
        self.assertEqual(host.journal, written, "the same declaration must write nothing")

        with self.assertRaises(SpecDrift) as drift:
            await self.declare(plan, command="true")
        self.assertEqual((drift.exception.key, sorted(drift.exception.diff)), ("tests", ["contract"]))
        with self.assertRaises(SpecDrift) as renamed:
            await plan.todo(key="freeze", label="freeze the flags")
        self.assertEqual(sorted(renamed.exception.diff), ["label"])
        self.assertEqual(host.journal, written, "a refused declaration must write nothing")

        self.assertEqual(plan["tests"].label, plan["write the test suite"].label)


if __name__ == "__main__":
    unittest.main()
