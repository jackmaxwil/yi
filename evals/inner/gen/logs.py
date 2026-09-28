"""Log forensics: logs too long to read in one tool result (the reducer cuts them), with incidents
planted among near-misses. Targets the large-output class: `pointer_never_read`,
`reduced_results` and `spiral_cut` fire on this shape in real and Terminal-Bench sessions.
Level 2 splits the events across two files merged by time; level 3 is larger and writes the edge
log's timestamps in a +02:00 offset, so file order is not time order across the two files."""
import datetime
import json
import random

SERVICES = ("auth", "billing", "catalog", "gateway", "search", "shipping")
EDGE = ("gateway", "search")
REQUESTS = {1: 1000, 2: 3000, 3: 6000}
INCIDENTS, RECOVERED = 8, 10
# Level 1 keeps the first version's clock (14:13:20Z), so its tasks are byte-identical to the
# ones the first inner A/A ran.
EPOCH = datetime.datetime(2026, 9, 28, 14, 13, 20, tzinfo=datetime.timezone.utc)


def _events(seed, level):
    rng = random.Random(f"logs:{seed}:{level}" if level > 1 else f"logs:{seed}")
    requests = [f"r{rng.randrange(16**6):06x}" for _ in range(REQUESTS[level])]
    incidents = set(rng.sample(requests, INCIDENTS))
    recovered = set(rng.sample([r for r in requests if r not in incidents], RECOVERED))
    events = []
    for request in requests:
        route = rng.sample(SERVICES, 3)
        for service in route:
            events.append((request, service, "INFO", "handled"))
        if request in incidents or request in recovered:
            events.append((request, rng.choice(route), "ERROR", "upstream timeout"))
            events.append((request, rng.choice(SERVICES), "WARN", "retrying attempt 2 of 3"))
            if request in incidents:
                events.append((request, rng.choice(SERVICES), "ERROR", "retry exhausted after 3 attempts"))
            else:
                events.append((request, rng.choice(SERVICES), "INFO", "retry succeeded, not exhausted"))
    rng.shuffle(events)
    # Position in the shuffled list is time: event i happens i seconds after the epoch.
    return [(EPOCH + datetime.timedelta(seconds=i), *event) for i, event in enumerate(events)]


def _stamp(when, local):
    if local:
        return when.astimezone(datetime.timezone(datetime.timedelta(hours=2))).isoformat()
    return when.strftime("%Y-%m-%dT%H:%M:%SZ")


def _files(seed, level):
    events = _events(seed, level)
    if level == 1:
        return {"service.log": "".join(f"{_stamp(t, False)} {s:<8} {lvl:<5} req={r} {x}\n"
                                       for t, r, s, lvl, x in events)}
    split = {"service.log": [], "edge.log": []}
    for t, r, s, lvl, x in events:
        name = "edge.log" if s in EDGE else "service.log"
        split[name].append(f"{_stamp(t, level == 3 and name == 'edge.log')} {s:<8} {lvl:<5} req={r} {x}\n")
    return {name: "".join(lines) for name, lines in split.items()}


def _expected(seed, level):
    first, exhausted = {}, set()
    for t, request, service, lvl, text in _events(seed, level):
        if lvl == "ERROR":
            first.setdefault(request, service)
        if "retry exhausted" in text:
            exhausted.add(request)
    return {request: first[request] for request in exhausted}


def make(seed, level=1):
    where = ("service.log in this directory is one day of logs from six services, one line per event, in time "
             "order." if level == 1 else
             "service.log and edge.log in this directory are one day of logs from six services, split by service "
             "(gateway and search write edge.log); each file is in time order, and one request's lines can be in "
             "both files. Order events by their timestamps, not by file.")
    prompt = (f"{where} A request is an incident when any line for it says `retry exhausted`. For each incident, "
              "find the service that logged that request's first `ERROR` line. Write incidents.json: a JSON list "
              "of objects {\"request\": \"r......\", \"service\": \"...\"}, one per incident, nothing else in the list.")
    return {"prompt": prompt, "files": _files(seed, level), "timeoutSec": 300 if level == 1 else 600}


def check(seed, workspace, level=1):
    """One point per incident named with its first-error service, one for listing nothing else."""
    expected = _expected(seed, level)
    try:
        got = json.loads((workspace / "incidents.json").read_text())
        pairs = {(item["request"], item["service"]) for item in got}
    except (OSError, ValueError, TypeError, KeyError):
        return 0, len(expected) + 1
    hits = sum(1 for pair in expected.items() if pair in pairs)
    clean = int(len(pairs) == len(expected) and hits == len(expected))
    return hits + clean, len(expected) + 1


def solve(seed, workspace, level=1):
    (workspace / "incidents.json").write_text(json.dumps(
        [{"request": r, "service": s} for r, s in sorted(_expected(seed, level).items())]))
