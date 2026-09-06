#!/usr/bin/env python3
"""The bundled catalogs against the world: models.dev and the providers' own lists. Drift is
an id a provider serves that the bundled floor lacks; the weekly job posts it to an issue.

    catalog_drift.py                 -> report on stdout, exit 1 on drift
    catalog_drift.py --issue         -> also upsert the report onto the "Catalog drift" issue
    catalog_drift.py --selfcheck
"""
import json, os, pathlib, sys, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
DATA = ROOT / "crates/ai/data"
PROVIDERS = ("anthropic", "openai", "openrouter")
MODELS_DEV = "https://models.dev/api.json"
OPENROUTER = "https://openrouter.ai/api/v1/models"
TITLE = "Catalog drift"


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


def report(missing):
    if not missing:
        return "Catalog drift: none — every id the providers serve is in the bundled floor.\n"
    lines = ["Catalog drift: the bundled floor lacks ids the providers serve.", ""]
    for provider, ids in sorted(missing.items()):
        lines.append(f"- `{provider}`: {len(ids)} — " + ", ".join(f"`{i}`" for i in ids[:12]) + (" …" if len(ids) > 12 else ""))
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
    print("ok   catalog_drift selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck(); return 0
    bundled = {p: bundled_ids(p) for p in PROVIDERS}
    api = fetch(MODELS_DEV)
    served = {p: set((api.get(p) or {}).get("models") or {}) for p in PROVIDERS}
    served["openrouter"] |= {row["id"] for row in fetch(OPENROUTER).get("data", []) if "id" in row}
    missing = drift(bundled, served)
    text = report(missing)
    sys.stdout.write(text)
    if "--issue" in argv:
        upsert_issue(text)
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
