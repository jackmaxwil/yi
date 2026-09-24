#!/usr/bin/env python3
"""Python in a prompt must run. An un-awaited coroutine call starts nothing and raises
nowhere. Incident: `h = rlm.run(...)` in identity.md cost a session 82 s of intended
parallelism and nine turns of API archaeology."""
import ast, re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

pkgs = sorted(ROOT.glob("python/yi_runtime/src/*/__init__.py"))
srcs = pkgs + sorted(ROOT.glob("python/skills/*/src/*/__init__.py"))
api = sorted({n for s in srcs for n in re.findall(r"async def (\w+)", s.read_text())})
mods = sorted({s.parent.name for s in srcs})
# A handle is any `*Handle` class, and every class of the `yi` library (Plan, Todo, Run).
classes = [c for p in pkgs for f in sorted(p.parent.glob("*.py")) for c in ast.parse(f.read_text()).body
           if isinstance(c, ast.ClassDef) and (c.name.endswith("Handle") or p.parent.name == "yi")]
handle = sorted({f.name for c in classes for f in c.body if isinstance(f, ast.AsyncFunctionDef)})
mod_call = re.compile(rf"(await\s+)?\b({'|'.join(mods)})\.({'|'.join(api)})\s*\(")
handle_call = re.compile(rf"(await\s+)?\w+\.({'|'.join(handle)})\s*\(")
kernel_block = re.compile(rf"{mod_call.pattern}|\b({'|'.join(c.name for c in classes)})\.\w+\(")

docs = (sorted(ROOT.glob("crates/runtime/src/prompts/*.md"))
        + sorted(ROOT.glob("skills/**/SKILL.md"))
        + sorted(ROOT.glob("python/skills/*/SKILL.md")))
errs = []
for d in docs:
    fenced, blocks, block = False, [], []
    for n, line in enumerate(d.read_text().splitlines(), 1):
        fence = line.lstrip().startswith("```")
        # A blank line does not close an indented block; the next indented line rejoins it.
        if not fence and (fenced or line.startswith("    ") or (block and not line.strip())):
            block.append((n, line))
            continue
        fenced = fenced != fence
        blocks.append(block)
        block = []
    blocks.append(block)
    # A handle is held in a variable, so `h.result(` parses the same as a Rust example's
    # `table.get(`. The block's own company is the discriminator.
    for b in blocks:
        pats = [mod_call, handle_call] if kernel_block.search("\n".join(l for _, l in b)) else [mod_call]
        errs += [f"{d.relative_to(ROOT)}:{n}: {m.group(0)}...) needs await"
                 for n, l in b for p in pats for m in p.finditer(l) if not m.group(1)]
fail(list(dict.fromkeys(errs)), f"prompt_examples ({len(api)} coroutines in {len(mods)} modules, "
                               f"{len(handle)} handle methods / {len(docs)} docs)")
