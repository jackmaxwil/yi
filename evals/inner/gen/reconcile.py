"""Reconcile two exports under stated rules: a multi-step spec task where decoys sit one rule
away from a match. Targets the multi-step class: intercepts, done-without-evidence and
multi-step-without-todo fire on this shape."""
import csv
import datetime
import io
import random
from pathlib import Path

ACCOUNTS = ("ops", "payroll", "rent", "travel", "vendors")
ROWS, MISSING_BANK, MISSING_LEDGER, DECOYS = 60, 4, 4, 4


def _world(seed):
    rng = random.Random(f"reconcile:{seed}")
    start = datetime.date(2026, 8, 1)
    ledger, bank = [], []
    for index in range(ROWS):
        account, cents = rng.choice(ACCOUNTS), rng.randrange(500, 500000)
        day = start + datetime.timedelta(days=rng.randrange(0, 50))
        ledger.append({"id": f"L{index:03d}", "account": account, "amount_cents": cents, "date": day})
        bank.append({"ref": f"B{index:03d}", "account": account, "amount": cents,
                     "posted": day + datetime.timedelta(days=rng.randrange(0, 3))})
    # Unmatched on the ledger side: drop their bank twins. Unmatched on the bank side: add bank-only rows.
    dropped = set(rng.sample(range(ROWS), MISSING_BANK))
    bank = [row for index, row in enumerate(bank) if index not in dropped]
    for extra in range(MISSING_LEDGER):
        bank.append({"ref": f"B9{extra:02d}", "account": rng.choice(ACCOUNTS), "amount": rng.randrange(500, 500000),
                     "posted": start + datetime.timedelta(days=rng.randrange(0, 50))})
    # Decoys: a ledger row whose only bank twin posted three days late, one past the rule, or to
    # another account; both sides stay unmatched.
    for decoy in range(DECOYS):
        account, cents = rng.choice(ACCOUNTS), rng.randrange(500, 500000)
        day = start + datetime.timedelta(days=rng.randrange(0, 45))
        ledger.append({"id": f"L8{decoy:02d}", "account": account, "amount_cents": cents, "date": day})
        other = account if decoy % 2 else next(a for a in ACCOUNTS if a != account)
        bank.append({"ref": f"B8{decoy:02d}", "account": other, "amount": cents,
                     "posted": day + datetime.timedelta(days=3 if decoy % 2 else 0)})
    rng.shuffle(bank)
    return ledger, bank


def _csv(rows, fields, fmt):
    out = io.StringIO()
    writer = csv.writer(out)
    writer.writerow(fields)
    for row in rows:
        writer.writerow([fmt(field, row[field]) for field in fields])
    return out.getvalue()


def _fmt(field, value):
    if field == "amount":
        return f"{value // 100}.{value % 100:02d}"
    if isinstance(value, datetime.date):
        return value.strftime("%m/%d/%Y") if field == "posted" else value.isoformat()
    return value


def _expected(seed):
    """Greedy one-to-one matching in ledger order: same account, same amount, posted 0 to 2 days
    after the ledger date; the earliest-posted candidate wins."""
    ledger, bank = _world(seed)
    free = list(bank)
    unmatched_ledger = []
    for row in ledger:
        candidates = [b for b in free if b["account"] == row["account"] and b["amount"] == row["amount_cents"]
                      and 0 <= (b["posted"] - row["date"]).days <= 2]
        if candidates:
            free.remove(min(candidates, key=lambda b: (b["posted"], b["ref"])))
        else:
            unmatched_ledger.append(row["id"])
    return {("unmatched_ledger", i) for i in unmatched_ledger} | {("unmatched_bank", b["ref"]) for b in free}


def make(seed):
    ledger, bank = _world(seed)
    prompt = ("ledger.csv is our books (amounts in integer cents, ISO dates); bank.csv is the bank's export "
              "(amounts in dollars with two decimals, dates as MM/DD/YYYY). A ledger row matches a bank row when "
              "the account is the same, the amount is the same, and the bank posted it 0 to 2 days after the "
              "ledger date. Each row matches at most one row on the other side; go through the ledger in file "
              "order and give each ledger row the earliest-posted bank row still free. Write report.csv with the "
              "header `kind,id` and one line per row that matched nothing: `unmatched_ledger,<ledger id>` or "
              "`unmatched_bank,<bank ref>`.")
    return {"prompt": prompt, "timeoutSec": 300, "files": {
        "ledger.csv": _csv(ledger, ["id", "account", "amount_cents", "date"], _fmt),
        "bank.csv": _csv(bank, ["ref", "account", "amount", "posted"], _fmt)}}


def check(seed, workspace):
    """One point per expected unmatched row, one for listing nothing else."""
    expected = _expected(seed)
    try:
        rows = list(csv.DictReader(io.StringIO((Path(workspace) / "report.csv").read_text())))
        got = {(row["kind"].strip(), row["id"].strip()) for row in rows}
    except (OSError, KeyError, csv.Error, AttributeError):
        return 0, len(expected) + 1
    hits = len(expected & got)
    return hits + int(got == expected), len(expected) + 1


def solve(seed, workspace):
    lines = ["kind,id"] + [f"{kind},{ident}" for kind, ident in sorted(_expected(seed))]
    (Path(workspace) / "report.csv").write_text("\n".join(lines) + "\n")
