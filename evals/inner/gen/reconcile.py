"""Reconcile two exports under stated rules: a multi-step spec task where decoys sit one rule
away from a match. Targets the multi-step class: intercepts, done-without-evidence and
multi-step-without-todo fire on this shape. Level 2 is larger and adds split payments (two bank
rows posted the same day that sum to one ledger row); level 3 adds EUR ledger rows converted with
a daily rate file."""
import csv
import datetime
import io
import random
from pathlib import Path

ACCOUNTS = ("ops", "payroll", "rent", "travel", "vendors")
# (rows, missing bank, missing ledger, decoys, splits) per level; level 1 is the first version's shape.
SHAPE = {1: (60, 4, 4, 4, 0), 2: (250, 6, 6, 6, 6), 3: (250, 6, 6, 6, 6)}
START = datetime.date(2026, 8, 1)


def _rate4(rates, day):
    return rates[day]


def _usd(row, rates):
    """A ledger row's amount in USD cents: EUR converts at the ledger date's rate, half up."""
    if row.get("currency", "USD") == "USD":
        return row["amount_cents"]
    return (row["amount_cents"] * rates[row["date"]] + 5000) // 10000


def _world(seed, level):
    rng = random.Random(f"reconcile:{seed}" if level == 1 else f"reconcile:{seed}:{level}")
    rows, missing_bank, missing_ledger, decoys, splits = SHAPE[level]
    rates = {START + datetime.timedelta(days=d): rng.randrange(10500, 11800) for d in range(60)} if level == 3 else {}
    ledger, bank = [], []
    for index in range(rows):
        account, cents = rng.choice(ACCOUNTS), rng.randrange(500, 500000)
        day = START + datetime.timedelta(days=rng.randrange(0, 50))
        row = {"id": f"L{index:03d}", "account": account, "amount_cents": cents, "date": day}
        if level == 3:
            row["currency"] = "EUR" if rng.random() < 0.25 else "USD"
        ledger.append(row)
        bank.append({"ref": f"B{index:03d}", "account": account, "amount": _usd(row, rates),
                     "posted": day + datetime.timedelta(days=rng.randrange(0, 3))})
    dropped = set(rng.sample(range(rows), missing_bank))
    split_at = set(rng.sample(sorted(set(range(rows)) - dropped), splits)) if splits else set()
    kept = []
    for index, row in enumerate(bank):
        if index in dropped:
            continue
        if index in split_at:
            first = rng.randrange(1, row["amount"])
            kept.append({**row, "ref": f"B{index:03d}a", "amount": first})
            kept.append({**row, "ref": f"B{index:03d}b", "amount": row["amount"] - first})
        else:
            kept.append(row)
    bank = kept
    for extra in range(missing_ledger):
        bank.append({"ref": f"B9{extra:02d}", "account": rng.choice(ACCOUNTS), "amount": rng.randrange(500, 500000),
                     "posted": START + datetime.timedelta(days=rng.randrange(0, 50))})
    for decoy in range(decoys):
        account, cents = rng.choice(ACCOUNTS), rng.randrange(500, 500000)
        day = START + datetime.timedelta(days=rng.randrange(0, 45))
        row = {"id": f"L8{decoy:02d}", "account": account, "amount_cents": cents, "date": day}
        if level == 3:
            row["currency"] = "USD"
        ledger.append(row)
        other = account if decoy % 2 else next(a for a in ACCOUNTS if a != account)
        bank.append({"ref": f"B8{decoy:02d}", "account": other, "amount": cents,
                     "posted": day + datetime.timedelta(days=3 if decoy % 2 else 0)})
    rng.shuffle(bank)
    return ledger, bank, rates


def _csv(rows, fields):
    out = io.StringIO()
    writer = csv.writer(out)
    writer.writerow(fields)
    for row in rows:
        writer.writerow([_fmt(field, row[field]) for field in fields])
    return out.getvalue()


def _fmt(field, value):
    if field == "amount":
        return f"{value // 100}.{value % 100:02d}"
    if isinstance(value, datetime.date):
        return value.strftime("%m/%d/%Y") if field == "posted" else value.isoformat()
    return value


def _expected(seed, level):
    """Singles first, greedy in ledger order: same account, same USD amount, posted 0 to 2 days after
    the ledger date, earliest-posted (then lowest ref) wins. From level 2, a ledger row left over then
    takes a pair of free bank rows on the same account posted the same day within that window whose
    amounts sum to it: the earliest-posted pair, then the lowest refs."""
    ledger, bank, rates = _world(seed, level)
    free, left = list(bank), []

    def window(b, row):
        return b["account"] == row["account"] and 0 <= (b["posted"] - row["date"]).days <= 2

    for row in ledger:
        want = _usd(row, rates)
        singles = [b for b in free if window(b, row) and b["amount"] == want]
        if singles:
            free.remove(min(singles, key=lambda b: (b["posted"], b["ref"])))
        else:
            left.append(row)
    unmatched = []
    for row in left:
        want = _usd(row, rates)
        pairs = [] if level == 1 else [
            (a, b) for i, a in enumerate(free) for b in free[i + 1:]
            if window(a, row) and window(b, row) and a["posted"] == b["posted"] and a["amount"] + b["amount"] == want]
        if pairs:
            a, b = min(pairs, key=lambda p: (p[0]["posted"], sorted((p[0]["ref"], p[1]["ref"]))))
            free.remove(a)
            free.remove(b)
        else:
            unmatched.append(row["id"])
    return {("unmatched_ledger", i) for i in unmatched} | {("unmatched_bank", b["ref"]) for b in free}


def make(seed, level=1):
    ledger, bank, rates = _world(seed, level)
    money = ("amounts in integer cents" if level < 3 else
             "amounts in integer cents of the row's currency, USD or EUR")
    prompt = (f"ledger.csv is our books ({money}, ISO dates); bank.csv is the bank's export "
              "(amounts in dollars with two decimals, dates as MM/DD/YYYY). A ledger row matches a bank row when "
              "the account is the same, the amount is the same, and the bank posted it 0 to 2 days after the "
              "ledger date. Each row matches at most one row on the other side; go through the ledger in file "
              "order and give each ledger row the earliest-posted bank row still free.")
    if level >= 2:
        prompt += (" Then, for each ledger row still unmatched, in file order: it matches two free bank rows on "
                   "the same account, posted on the same day, 0 to 2 days after the ledger date, whose amounts sum "
                   "to it; if several pairs qualify take the earliest-posted pair, then the one with the lowest refs.")
    if level == 3:
        prompt += (" Compare in USD: convert an EUR ledger amount with rates.csv's rate for the ledger date "
                   "(USD per EUR, four decimals), rounding to the nearest cent, halves up.")
    prompt += (" Write report.csv with the header `kind,id` and one line per row that matched nothing: "
               "`unmatched_ledger,<ledger id>` or `unmatched_bank,<bank ref>`.")
    fields = ["id", "account", "amount_cents", "date"] + (["currency"] if level == 3 else [])
    files = {"ledger.csv": _csv(ledger, fields), "bank.csv": _csv(bank, ["ref", "account", "amount", "posted"])}
    if level == 3:
        files["rates.csv"] = "date,usd_per_eur\n" + "".join(
            f"{day.isoformat()},{rate // 10000}.{rate % 10000:04d}\n" for day, rate in sorted(rates.items()))
    return {"prompt": prompt, "timeoutSec": 300 if level == 1 else 600, "files": files}


def check(seed, workspace, level=1):
    """One point per expected unmatched row, one for listing nothing else."""
    expected = _expected(seed, level)
    try:
        rows = list(csv.DictReader(io.StringIO((Path(workspace) / "report.csv").read_text())))
        got = {(row["kind"].strip(), row["id"].strip()) for row in rows}
    except (OSError, KeyError, csv.Error, AttributeError):
        return 0, len(expected) + 1
    hits = len(expected & got)
    return hits + int(got == expected), len(expected) + 1


def solve(seed, workspace, level=1):
    lines = ["kind,id"] + [f"{kind},{ident}" for kind, ident in sorted(_expected(seed, level))]
    (Path(workspace) / "report.csv").write_text("\n".join(lines) + "\n")
