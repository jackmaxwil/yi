"""F1a: `help(yi)` is the documentation, so every example in it has to be right (plan section 8.7)."""
from __future__ import annotations

import ast
import inspect
import textwrap
import unittest

import yi

HANDLES = {"Plan": yi.Plan, "plan": yi.Plan, "sub": yi.Plan, "todo": yi.Todo, "run": yi.Run}


def example_of(thing: object) -> str:
    """The docstring's last paragraph, which must be Python (3.13 strips its indentation)."""
    last = textwrap.dedent((inspect.getdoc(thing) or "").split("\n\n")[-1])
    try:
        compile(last, "<example>", "exec", ast.PyCF_ONLY_AST | ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    except SyntaxError as error:
        raise AssertionError(f"the docstring must end with an example: {error}") from error
    return last


def problems(example: str) -> list[str]:
    """Names that do not exist, and calls whose await does not match the callee's kind."""
    tree = compile(example, "<example>", "exec", ast.PyCF_ONLY_AST | ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    awaited = {id(node.value) for node in ast.walk(tree) if isinstance(node, ast.Await)}
    found = []
    for call in (node for node in ast.walk(tree) if isinstance(node, ast.Call)):
        func = call.func
        if isinstance(func, ast.Name) and func.id in yi.__all__:
            target, name = getattr(yi, func.id), func.id
        elif isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) and func.value.id in HANDLES:
            name = f"{func.value.id}.{func.attr}"
            target = getattr(HANDLES[func.value.id], func.attr, None)
            if target is None:
                found.append(f"{name} does not exist")
                continue
        else:
            continue
        if inspect.iscoroutinefunction(target) != (id(call) in awaited):
            found.append(f"{name} is {'a coroutine' if inspect.iscoroutinefunction(target) else 'synchronous'}")
    return found


class Help(unittest.TestCase):
    def documented(self) -> dict[str, object]:
        names = {name: getattr(yi, name) for name in yi.__all__}
        for cls in (yi.Plan, yi.Todo, yi.Run):
            for name, member in vars(cls).items():
                if not name.startswith("_") and (inspect.isfunction(member) or isinstance(member, classmethod)):
                    names[f"{cls.__name__}.{name}"] = getattr(cls, name)
        return names

    def test_every_name_in_all_has_a_docstring_with_a_valid_example(self) -> None:
        names = self.documented()
        self.assertGreater(len(names), len(yi.__all__) + 15, "the methods are documented names too")
        for name, thing in names.items():
            with self.subTest(name):
                example = example_of(thing)
                # A Todo or a Run is handed out, never constructed, so its example names the handle.
                self.assertIn(name.split(".")[-1].lower(), example.lower(), "the example must use its name")
                self.assertEqual(problems(example), [])
        self.assertEqual(problems(example_of(yi)), [])

    def test_the_gate_rejects_a_wrong_example(self) -> None:
        self.assertEqual(problems('todo = plan.todo(key="a")'), ["plan.todo is a coroutine"])
        self.assertEqual(problems('check = await cmd("true")'), ["cmd is synchronous"])
        self.assertEqual(problems("await plan.frobnicate()"), ["plan.frobnicate does not exist"])
        self.assertEqual(problems("for todo in plan.ready():\n    await todo.start()"), [])
        with self.assertRaises(AssertionError):
            example_of(problems)


if __name__ == "__main__":
    unittest.main()
