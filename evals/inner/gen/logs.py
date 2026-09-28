"""Log forensics: a log too long to read in one tool result (the reducer cuts it), with incidents
planted among near-misses. Targets the large-output class: `pointer_never_read`,
`reduced_results` and `spiral_cut` fire on this shape in real and Terminal-Bench sessions."""
import json
import random

SERVICES = ("auth", "billing", "catalog", "gateway", "search", "shipping")
LINES, INCIDENTS, RECOVERED = 6000, 8, 10


def _world(seed):
    rng = random.Random(f"logs:{seed}")
    requests = [f"r{rng.randrange(16**6):06x}" for _ in range(LINES // 6)]
    incidents = rng.sample(requests, INCIDENTS)
    recovered = rng.sample([r for r in requests if r not in incidents], RECOVERED)
    events = []
    for request in requests:
        route = rng.sample(SERVICES, 3)
        for service in route:
            events.append((request, service, "INFO", "handled"))
        if request in incidents or request in recovered:
            first = rng.choice(route)
            events.append((request, first, "ERROR", "upstream timeout"))
            events.append((request, rng.choice(SERVICES), "WARN", "retrying attempt 2 of 3"))
            if request in incidents:
                events.append((request, rng.choice(SERVICES), "ERROR", "retry exhausted after 3 attempts"))
            else:
                events.append((request, rng.choice(SERVICES), "INFO", "retry succeeded, not exhausted"))
    rng.shuffle(events)
    stamp = 1790000000
    lines = []
    for index, (request, service, level, text) in enumerate(events):
        lines.append(f"2026-09-28T{(stamp + index) % 86400 // 3600:02d}:{(stamp + index) % 3600 // 60:02d}:"
                     f"{(stamp + index) % 60:02d}Z {service:<8} {level:<5} req={request} {text}")
    return "\n".join(lines) + "\n"


def _first_errors(log):
    first = {}
    for line in log.splitlines():
        parts = line.split()
        if parts[2] == "ERROR":
            first.setdefault(parts[3][4:], parts[1])
    return first


def make(seed):
    log = _world(seed)
    prompt = ("service.log in this directory is one day of logs from six services, one line per event, "
              "in time order. A request is an incident when any line for it says `retry exhausted`. For each "
              "incident, find the service that logged that request's first `ERROR` line. Write incidents.json: "
              "a JSON list of objects {\"request\": \"r......\", \"service\": \"...\"}, one per incident, "
              "nothing else in the list.")
    return {"prompt": prompt, "files": {"service.log": log}, "timeoutSec": 300}


def _expected(seed):
    log = _world(seed)
    first = _first_errors(log)
    exhausted = {line.split()[3][4:] for line in log.splitlines() if "retry exhausted" in line}
    return {request: first[request] for request in exhausted}


def check(seed, workspace):
    """One point per incident named with its first-error service, one for listing nothing else."""
    expected = _expected(seed)
    try:
        got = json.loads((workspace / "incidents.json").read_text())
        pairs = {(item["request"], item["service"]) for item in got}
    except (OSError, ValueError, TypeError, KeyError):
        return 0, len(expected) + 1
    hits = sum(1 for pair in expected.items() if pair in pairs)
    clean = int(len(pairs) == len(expected) and hits == len(expected))
    return hits + clean, len(expected) + 1


def solve(seed, workspace):
    (workspace / "incidents.json").write_text(json.dumps(
        [{"request": r, "service": s} for r, s in sorted(_expected(seed).items())]))
