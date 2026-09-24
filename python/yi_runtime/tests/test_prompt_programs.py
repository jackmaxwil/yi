"""F1d: every `yi` program a prompt fragment teaches runs to `verified_success` on the fake host."""
from __future__ import annotations

import ast
import inspect
import pathlib
import unittest

import yi.plan
from fake_host import FakeHost

PROMPTS = pathlib.Path(__file__).resolve().parents[3] / "crates/runtime/src/prompts"


def programs() -> list[tuple[str, str]]:
    """The indented blocks of every fragment that import `yi`, dedented."""
    found = []
    for path in sorted(PROMPTS.glob("*.md")):
        block: list[str] = []
        for line in path.read_text().splitlines() + [""]:
            if line.startswith("    ") or (block and not line.strip()):
                block.append(line[4:])
                continue
            source = "\n".join(block).strip()
            if "from yi import" in source:
                found.append((f"{path.name}:{len(found)}", source))
            block = []
    return found


class Landing(FakeHost):
    """Every child lands at once; a reader quotes a line that is on its own partition."""

    def step(self, op, args, plan, todo):
        refused = super().step(op, args, plan, todo)
        if op == "start" and refused is None and todo.get("delegation"):
            quotes = [{"url": f"{root}/lib.rs", "line": 1, "text": "mint"} for root in todo["delegation"].get("context", [])]
            self.files.update({quote["url"]: "fn mint() {}\n" for quote in quotes})
            self.children[todo["by"]] = "finished"
            self.results[todo["by"]] = {"text": "", "json": {"answer": "in mint()", "quotes": quotes}}
        return refused


class PromptPrograms(unittest.IsolatedAsyncioTestCase):
    def tearDown(self) -> None:
        yi.plan._RUNS.clear()

    async def test_every_yi_program_in_a_prompt_runs_to_verified_success(self) -> None:
        found = programs()
        self.assertGreaterEqual(len(found), 2, "orchestrate.md teaches a reader and a writer program")
        for name, source in found:
            with self.subTest(name):
                host = Landing()
                names: dict = {}
                pending = eval(compile(source, name, "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT), names)
                if inspect.iscoroutine(pending):
                    await pending
                self.assertEqual((names["run"].outcome, names["run"].refusals), ("verified_success", {}))
                self.assertGreaterEqual(host.spawns, 2)


if __name__ == "__main__":
    unittest.main()
