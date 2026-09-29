#!/usr/bin/env python3
"""The bundled catalogs against the world: models.dev and the providers' own lists. Drift is
an id a provider serves that the bundled floor lacks, and such an id whose derived request
facts differ from its nearest bundled sibling's; the weekly job posts it to an issue.

    catalog_drift.py                 -> report on stdout, exit 1 on drift
    catalog_drift.py --issue         -> also upsert the report onto the "Catalog drift" issue
    catalog_drift.py --selfcheck
"""
import json, os, pathlib, re, sys, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
DATA = ROOT / "crates/ai/data"
PROVIDERS = ("anthropic", "openai", "openrouter")
MODELS_DEV = "https://models.dev/api.json"
OPENROUTER = "https://openrouter.ai/api/v1/models"
TITLE = "Catalog drift"
COMPAT = ROOT / "crates/ai/src/compat.rs"


def fetch(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.loads(response.read())


def bundled_ids(provider):
    data = json.loads((DATA / f"{provider}.json").read_text())
    return {model["id"] for models in data.values() for model in models.values()}


def drift(bundled, served):
    """Per provider: the ids served that the bundle lacks, sorted; an empty map is no drift."""
    return {provider: sorted(set(served.get(provider, ())) - set(bundled.get(provider, ())))
            for provider in bundled if set(served.get(provider, ())) - set(bundled.get(provider, ()))}


def reasoning_rows(source=None):
    """`REASONING_CONTENT` as compat.rs spells it, so this report reads the table the binary does."""
    table = (source or COMPAT.read_text()).split("REASONING_CONTENT", 1)[1].split("];", 1)[0]
    return [(prefix, flag == "true") for prefix, flag in re.findall(r'\("([^"]+)", (true|false)\)', table)]


def facts(model_id, rows):
    """compat.rs's id-derived facts: reasoning-content replay (first matching row) and adaptive
    thinking (a Claude generation from 4.6, date suffixes not being versions)."""
    bare = model_id.lstrip("~")
    replay = next((flag for prefix, flag in rows if bare.startswith(prefix)), False)
    numbers = [int(p) for p in bare.removeprefix("claude-").split("-") if p.isdigit() and int(p) < 100]
    generation = (numbers[0], (numbers + [0])[1]) if bare.startswith("claude-") and numbers else None
    return {"reasoning_content": replay, "adaptive_thinking": generation is None or generation >= (4, 6)}


def sibling(model_id, floor):
    """The bundled id nearest `model_id`: the longest common prefix, then the closest version
    number after it (`kimi-k2.8` reads `kimi-k2.7-code`, not `kimi-k2.5`), then the shortest id."""
    if not floor:
        return None
    best = max(len(os.path.commonprefix([model_id, b])) for b in floor)
    number = lambda i: int(m.group(1)) if (m := re.match(r"[.\-_]?(\d+)", i[best:])) else None
    mine = number(model_id)
    gap = lambda b: abs(mine - number(b)) if mine is not None and number(b) is not None else float("inf")
    return min((b for b in floor if len(os.path.commonprefix([model_id, b])) == best), key=lambda b: (gap(b), len(b), b))


def fact_drift(bundled, missing, rows):
    """Per provider: each missing id whose facts differ from its nearest bundled sibling's."""
    out = {}
    for provider, ids in missing.items():
        floor = sorted(bundled.get(provider, ()))
        for model_id in ids:
            sibling_id = sibling(model_id, floor)
            if sibling_id and facts(model_id, rows) != facts(sibling_id, rows):
                out.setdefault(provider, []).append((model_id, sibling_id))
    return out


def report(missing, differ=None):
    if not missing:
        return "Catalog drift: none — every id the providers serve is in the bundled floor.\n"
    lines = ["Catalog drift: the bundled floor lacks ids the providers serve.", ""]
    for provider, ids in sorted(missing.items()):
        lines.append(f"- `{provider}`: {len(ids)} — " + ", ".join(f"`{i}`" for i in ids[:12]) + (" …" if len(ids) > 12 else ""))
    if differ:
        lines += ["", "Derived facts that differ from the nearest bundled sibling (check the rows in `crates/ai/src/compat.rs`):"]
        for provider, pairs in sorted(differ.items()):
            lines += [f"- `{provider}`: `{i}` vs `{s}`" for i, s in pairs]
    lines += ["", "`yi catalog refresh` covers a session; the floor moves with `crates/ai/data/*.json`."]
    return "\n".join(lines) + "\n"


def upsert_issue(text):
    sys.path.insert(0, str(ROOT / "scripts"))
    from forgejo_pr_comment import send  # noqa: E402
    api, repo = os.environ["FORGEJO_API_URL"], os.environ["FORGEJO_REPOSITORY"]
    found = [i for i in send("GET", f"{api}/repos/{repo}/issues?type=issues&state=open&q={TITLE.replace(' ', '+')}") or []
             if i.get("title") == TITLE]
    if found:
        send("POST", f"{api}/repos/{repo}/issues/{found[0]['number']}/comments", {"body": text})
    else:
        send("POST", f"{api}/repos/{repo}/issues", {"title": TITLE, "body": text})


def selfcheck():
    bundled = {"openai": ["a", "b"], "openrouter": ["x/y"]}
    served = {"openai": ["a", "b", "c"], "openrouter": ["x/y"], "anthropic": ["ignored"]}
    assert drift(bundled, served) == {"openai": ["c"]}
    assert drift(bundled, {"openai": ["a"]}) == {}, "a served subset is no drift"
    text = report({"openai": ["c"]})
    assert "`openai`: 1 — `c`" in text and "refresh" in text
    assert report({}).startswith("Catalog drift: none")
    rows = reasoning_rows()
    assert ("deepseek/", True) in rows and ("deepseek/deepseek-r1", False) in rows, rows
    floor = {"openrouter": ["deepseek/deepseek-r1", "deepseek/deepseek-v4-pro"], "anthropic": ["claude-haiku-4-5"]}
    new = {"openrouter": ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-r2"], "anthropic": ["claude-haiku-5", "claude-haiku-4-5-20251001"]}
    assert fact_drift(floor, new, rows) == {
        "openrouter": [("deepseek/deepseek-r2", "deepseek/deepseek-r1")],
        "anthropic": [("claude-haiku-5", "claude-haiku-4-5")],
    }, fact_drift(floor, new, rows)
    assert "`deepseek/deepseek-r2` vs `deepseek/deepseek-r1`" in report(new, fact_drift(floor, new, rows))
    kimi = ["moonshotai/kimi-k2-thinking", "moonshotai/kimi-k2.5", "moonshotai/kimi-k2.6",
            "moonshotai/kimi-k2.7-code", "moonshotai/kimi-k2.7-code:batch", "moonshotai/kimi-k3"]
    assert sibling("moonshotai/kimi-k2.8", kimi) == "moonshotai/kimi-k2.7-code", "a tie goes to the closest version"
    assert sibling("moonshotai/kimi-k4", kimi) == "moonshotai/kimi-k3"
    tied = {"openrouter": ["deepseek/deepseek-r1", "deepseek/deepseek-r5"]}
    assert fact_drift(tied, {"openrouter": ["deepseek/deepseek-r4"]}, rows) == {}, "r4 reads r5, not r1 by sort order"
    print("ok   catalog_drift selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck(); return 0
    bundled = {p: bundled_ids(p) for p in PROVIDERS}
    api = fetch(MODELS_DEV)
    served = {p: set((api.get(p) or {}).get("models") or {}) for p in PROVIDERS}
    served["openrouter"] |= {row["id"] for row in fetch(OPENROUTER).get("data", []) if "id" in row}
    missing = drift(bundled, served)
    text = report(missing, fact_drift(bundled, missing, reasoning_rows()))
    sys.stdout.write(text)
    if "--issue" in argv:
        upsert_issue(text)
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
