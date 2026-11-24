"""Turns the page's data tables and the scraped details into row fixtures in the shape docs/conventions.md gives."""
import html
import re


def plain(s):
    return html.unescape(re.sub(r"<[^>]+>", "", str(s))).strip()


def snake(s):
    return re.sub(r"[^a-z0-9]+", "_", plain(s).lower()).strip("_")


def t(x):
    return {"text": plain(x)}


def dot(state):
    return {"dot": state}


def badge(text, tone):
    return {"badge": [plain(text), tone]}


def pill(text, tone):
    return {"pill": [plain(text), tone]}


def tags(items):
    return {"tags": [plain(i) for i in items]}


def meter(ratio, text):
    return {"meter": {"ratio": round(ratio, 4), "text": text}}


def bar(start, length, tone):
    return {"bar": {"start": round(start, 4), "len": round(length, 4), "tone": tone}}


def flag(b):
    return {"flag": bool(b)}


class Rows:
    """Collects the rows of one pane and the facts that do not fit a row: the open row, group notes."""

    def __init__(self):
        self.rows, self.open, self.notes, self.selected = [], None, {}, None

    def id_for(self, name):
        base = re.sub(r"[^a-z0-9]+", "-", plain(name).lower()).strip("-") or "row"
        taken = {r["id"] for r in self.rows}
        out, n = base, 2
        while out in taken:
            out, n = f"{base}-{n}", n + 1
        return out

    def add(self, name, cells, parent=None, group=None, dim=False, tone=None, rid=None):
        row = {"id": rid or self.id_for(name)}
        if parent:
            row["parent"] = parent
        if group:
            row["group"] = group
        if dim:
            row["dim"] = True
        if tone:
            row["tone"] = tone
        if group and self.notes.get(group) and not any(r.get("group") == group for r in self.rows):
            cells = {**cells, "group.summary": t(self.notes[group])}
        row["cells"] = cells
        self.rows.append(row)
        return row["id"]

    def meta(self):
        m = {}
        if self.open:
            m["open"] = self.open
        if self.selected:
            m["selected"] = self.selected
        if self.notes:
            m["group_notes"] = self.notes
        return m


def put(cells, key, value):
    if key not in cells:
        cells[key] = value
    elif cells[key] != value:
        cells[key + "_detail"] = value


def detail(cells, d, buttons=True, skip=()):
    """Every fact of an opened detail goes into the row under its own label."""
    if not d:
        return
    for label, value in d["kv"] + d["kvl"]:
        if snake(label) not in skip:
            put(cells, snake(label), t(value))
    if d["pre"]:
        put(cells, "json", t(d["pre"]))
    if d["note"]:
        put(cells, "note", t(d["note"]))
    if buttons and d["buttons"]:
        put(cells, "actions", tags([b["text"] for b in d["buttons"]]))


STATE_TONE = {"new": "accent", "open": "dim", "fix": "accent", "prove": "need", "watch": "ok", "done": "ok"}
SEV = {"H": ("High", "bad"), "M": ("Med", "warn"), "L": ("Low", "dim")}
LEVEL_TONE = {"I": "dim", "D": "faint", "W": "warn", "E": "bad"}
SPAN_TONE = {"turn": "dim", "model": "model", "tool": "ok", "child": "accent", "wait": "need"}
KIND_TONE = {"k-turn": "dim", "k-child": "accent", "k-wait": "need", "k-model": "model", "k-tool": "ok"}
CHECK = {"y": flag(True), "n": flag(False), "o": t("–")}


def parented(depths):
    """Parent index for each depth in a flat list that nests by depth."""
    stack, out = [], []
    for i, d in enumerate(depths):
        out.append(stack[d - 1] if d else None)
        stack[d:] = [i]
    return out


def agents(raw):
    AG, det = raw["data"]["AG"], raw["scrapes"]["agent_detail"]
    r, ids = Rows(), []
    for a, parent in zip(AG, parented([a[0] for a in AG])):
        depth, state, name, doing, model, tokens, cost, time, (s, e), kind = a[:10]
        cells = {"agent": t(name), "state": dot(state), "doing_now": t(doing), "model": badge(model, "model"),
                 "tokens": t(tokens), "cost": t(cost), "time": t(time),
                 "last_14_min": bar(s / 14, max(0.03, (e - s) / 14), KIND_TONE[kind])}
        if len(a) > 10 and a[10]:
            for label, v in det["pairs"]:
                cells[snake(label)] = tags(v["tags"]) if "tags" in v else meter(v["ratio"], v["text"]) if "ratio" in v else t(v["text"])
            cells["actions"] = tags([b["text"] for b in det["buttons"]])
        ids.append(r.add(name, cells, parent=ids[parent] if parent is not None else None))
        if len(a) > 10 and a[10]:
            r.open = ids[-1]
    return r


def trace(raw):
    SP, det = raw["data"]["SP"], raw["scrapes"]["detail:trace"]
    r, ids = Rows(), []
    for s, d, parent in zip(SP, det, parented([s[0] for s in SP])):
        _, name, start, dur, kind, opened = s[:6]
        took = "0:42…" if kind == "wait" else (f"{round(dur * 1000)} ms" if dur < 1 else f"{dur:.1f} s")
        cells = {"step": t(name), "start": t(f"+{start:.1f}" if start else "0"), "took": t(took),
                 "timeline": bar(start / 15, dur / 15, SPAN_TONE[kind])}
        detail(cells, d)
        ids.append(r.add(name, cells, parent=ids[parent] if parent is not None else None))
        if opened:
            r.open = ids[-1]
    return r


def logs(raw):
    LG, LV, det = raw["data"]["LG"], raw["data"]["LV"], raw["scrapes"]["detail:logs"]
    r = Rows()
    for i, (l, d) in enumerate(zip(LG, det)):
        cells = {"time": t(l[0]), "level": badge(LV[l[1]], LEVEL_TONE[l[1]]), "source": t(l[2]), "message": t(l[3])}
        detail(cells, d, skip=("time", "level", "source"))
        r.add(l[0], cells, tone=LEVEL_TONE[l[1]] if l[1] in "WE" else None, rid=l[0])
        if i == 10:
            r.open = l[0]
    return r


def findings_table(raw):
    FD, det = raw["data"]["FD"], raw["scrapes"]["detail:findings-table"]
    r = Rows()
    for f, d in zip(FD, det):
        sev, tone = SEV[f[0]]
        cells = {"severity": badge(sev, tone), "finding": t(f[1]), "where": t(f[2]), "state": pill(f[4], STATE_TONE[f[3]]),
                 "who_has_it": t(f[5]), "age": t(f[6]), "why": t(f[7]), "proof": t(f[8]), "proof_state": t(f[9]),
                 "evidence": tags(d["caps"]), "actions": tags([b["text"] for b in d["buttons"]])}
        rid = r.add(f[1], cells)
        if len(f) > 13 and f[13]:
            r.open = rid
    return r


def findings(raw):
    hub, det, table = raw["scrapes"]["hub"], raw["scrapes"]["finding_detail"], findings_table(raw)
    by_title = {row["cells"]["finding"]["text"]: row for row in table.rows}
    r, group = Rows(), None
    for item in hub["items"]:
        if item["type"] == "group":
            group = item["title"]
            r.notes[group] = " · ".join(x for x in (item["note"], item["extra"]) if x)
            continue
        tone = SEV[item["sev"]][1]
        cells = {"severity": badge(item["sv"], tone), "finding": t(item["title"])}
        if item["state"]:
            cells["state"] = pill(item["state"]["label"], STATE_TONE[item["state"]["cls"]])
        cells["meta"] = tags([m["text"] for m in item["meta"]])
        spark = [m["spark"] for m in item["meta"] if m["spark"]]
        if spark:
            cells["seen_series"] = {"series": spark[0]}
        blocks = [m["text"] for m in item["meta"] if m["blocks"]]
        if blocks:
            cells["blocks"] = t(blocks[0])
        if item["buttons"]:
            cells["actions"] = tags(item["buttons"])
        known = by_title.get(item["title"])
        if known:
            for k in ("where", "who_has_it", "age", "why", "proof", "proof_state", "evidence"):
                cells[k] = known["cells"][k]
            cells["actions"] = known["cells"]["actions"]
        rid = r.add(item["title"], cells, group=group)
        if item["on"] and det["title"] == item["title"]:
            r.open = rid
            cells["finding_id"] = t(det["id"])
            cells["flags"] = tags(det["chips"][1:-1])
            for lab, note in ((x["label"], x["note"]) for x in det["labels"]):
                if lab == "Proposed fix":
                    cells["proposed_fix_by"] = t(note)
            cells["proposed_fix"] = tags([f"{'-' if x['sign'] == 'm' else '+'} {x['text'][2:]}" for x in det["fix"]])
            cells["proof_checks"] = tags([f"{c['text']} · {c['note']}" for c in det["checks"]])
            cells["history"] = tags([f"{a} {b}" for a, b in zip(det["history"][::2], det["history"][1::2])])
            cells["actions"] = tags(det["buttons"])
    return r


def assets(raw):
    AS, det = raw["data"]["AS"], raw["scrapes"]["detail:assets"]
    r, group, k = Rows(), None, 0
    for a in AS:
        if len(a) == 2:
            group = a[0]
            r.notes[group] = a[1]
            continue
        cells = {"state": dot(a[0]), "asset": t(a[1]), "about": t(a[2]), "reach": CHECK[a[3]], "logs": CHECK[a[4]],
                 "backup": CHECK[a[5]], "scan": CHECK[a[6]], "seen": t(a[7])}
        detail(cells, det[k], skip=("what",))
        rid = r.add(a[1], cells, group=group)
        if a[1] == "build-01":
            r.open = rid
        k += 1
    return r


def containers(raw):
    CTS, det = raw["data"]["CTS"], raw["scrapes"]["detail:containers"]
    r, group, k = Rows(), None, 0
    for c in CTS:
        if len(c) == 2:
            group = c[0] or "stopped"
            r.notes[group] = c[1]
            continue
        cells = {"state": dot(c[0]), "name": t(c[1]), "image": t(c[2]), "cpu": meter(c[3] / 100, f"{c[3]}%"),
                 "mem": t(c[4]), "ports": t(c[5]), "up": t(c[6])}
        detail(cells, det[k], skip=("image", "ports"))
        rid = r.add(c[1], cells, group=group, dim=c[0] == "fail")
        if len(c) > 7 and c[7]:
            r.open = rid
        k += 1
    return r


def vms(raw):
    VMS, det = raw["data"]["VMS"], raw["scrapes"]["detail:vms"]
    r = Rows()
    for v, d in zip(VMS, det):
        cells = {"state": dot(v[0]), "name": t(v[1]), "cpu": t(v[2]), "memory": meter(v[3] / 100, f"{v[3]}% of {v[4]}") if v[3] else t(v[4]),
                 "disk": t(v[5]), "address": t(v[6]), "snaps": t(v[7]), "up": t(v[8])}
        detail(cells, d)
        rid = r.add(v[1], cells, dim=v[0] == "stop")
        if len(v) > 9 and v[9]:
            r.open = rid
    return r


def daemons(raw):
    DMS, det = raw["data"]["DMS"], raw["scrapes"]["detail:daemons"]
    r = Rows()
    for m, d in zip(DMS, det):
        cells = {"state": dot(m[0]), "name": t(m[1]), "what_it_is": t(m[2]), "pid": t(m[3]), "up": t(m[4]),
                 "restarts": t(m[5]), "login": flag(m[6] == "y")}
        detail(cells, d, skip=("what", "pid", "up", "restarts"))
        rid = r.add(m[1], cells, dim=m[0] == "stop")
        if len(m) > 7 and m[7]:
            r.open = rid
    return r


def lsp(raw):
    LSV, LSD, det = raw["data"]["LSV"], raw["data"]["LSD"], raw["scrapes"]["detail:lsp"]
    r = Rows()
    for s, d in zip(LSV, det):
        cells = {"state": dot(s[0]), "server": t(s[1]), "language": t(s[2]), "project": t(s[3]), "memory": t(s[4]),
                 "status": t(d["lead"]), "can_do": tags(d["caps"]), "agent_may_ask": flag(d["toggle"]),
                 "actions": tags([b["text"] for b in d["buttons"]])}
        rid = r.add(s[1], cells, dim=s[0] == "stop")
        if len(s) > 5 and s[5]:
            r.open = rid
    return r


def problems(raw):
    DG, det = raw["data"]["DG"], raw["scrapes"]["detail:problems"]
    tone = {"Error": "bad", "Warn": "warn", "Hint": "dim"}
    r = Rows()
    for i, (g, d) in enumerate(zip(DG, det)):
        cells = {"severity": badge(g[1], tone[g[1]]), "message": t(g[2]), "where": t(g[3])}
        detail(cells, d)
        rid = r.add(g[3], cells)
        if i == 1:
            r.open = rid
    return r


def plugins(raw):
    PL, det = raw["data"]["PL"], raw["scrapes"]["detail:plugins"]
    r = Rows()
    for p, d in zip(PL, det):
        cells = {"letter": t(p[0]), "color": t(p[1]), "name": t(p[2]), "version": t(p[3]), "enabled": flag(p[5] == "y"),
                 "adds": tags(p[4]), "update": flag(len(p) > 7 and p[7] == "update"), "needs_attention": flag(len(p) > 7 and p[7] == "warn"),
                 "description": t(d["lead"]), "source": t(d["note"]),
                 "allowed": tags([a for a, ok in d["perms"] if ok]), "denied": tags([a for a, ok in d["perms"] if not ok]),
                 "actions": tags([b["text"] for b in d["buttons"]])}
        rid = r.add(p[2], cells, dim=p[5] == "n")
        if len(p) > 6 and p[6]:
            r.open = rid
    return r


def debugger(raw):
    D, data = raw["scrapes"]["debugger"], raw["data"]
    secs = list(D.values())
    var, watch, stack, bps = Rows(), Rows(), Rows(), Rows()
    ids = []
    for v, parent, s in zip(data["DV"], parented([v[0] for v in data["DV"]]), secs[0]):
        cells = {"name": t(v[2]), "value": t(v[3]), "type": t(v[4]), "changed": flag(v[5])}
        detail(cells, s["detail"], skip=("type",))
        ids.append(var.add(v[2], cells, parent=ids[parent] if parent is not None else None))
        if v[5]:
            var.open = ids[-1]
    for w, s in zip(data["DW"], secs[1]):
        cells = {"expression": t(w[0]), "value": t(w[1])}
        detail(cells, s["detail"], skip=("result",))
        watch.add(w[0], cells)
    for f, s in zip(data["DS"], secs[2]):
        cells = {"frame": t(f[0]), "function": t(f[1]), "at": t(f[2]), "current": flag(f[3])}
        detail(cells, s["detail"], skip=("function", "at"))
        rid = stack.add(f[0] + " " + f[1], cells)
        if f[3]:
            stack.open = rid
    for extra in secs[2][len(data["DS"]):]:
        stack.add("thread 2", {"frame": t(extra["text"]), "function": t(""), "at": t(""), "current": flag(False)}, dim=True)
    for b, s in zip(data["DB"], secs[3]):
        cells = {"enabled": flag(b[0] == "●"), "where": t(b[1]), "condition": t(b[2]), "hits": t(b[3])}
        detail(cells, s["detail"])
        bps.add(b[1], cells, dim=bool(b[4]))
    return {"debugger-variables": var, "debugger-watch": watch, "debugger-stack": stack, "debugger-breakpoints": bps}


def settings(raw):
    S = raw["scrapes"]["settings"]["rows"]
    r = Rows()
    for s in S:
        c = s["control"]
        value = (flag(c["flag"]) if "flag" in c else t(c["choice"]) if "choice" in c else meter(c["ratio"], c["text"]) if "ratio" in c
                 else t(c["select"]) if "select" in c else t(c["text"]))
        cells = {"setting": t(s["title"]), "description": t(s["desc"]), "key": t(s["key"]), "value": value, "set_by": t(s["scope"]),
                 "changed": flag(s["mod"])}
        r.add(s["key"], cells, rid=s["key"])
    return r


def themes(raw):
    TH = raw["data"]["THEMES"]
    r = Rows()
    for k, v in TH.items():
        r.add(k, {"theme": t(v["n"]), "note": t(v["d"]), "swatches": tags(v["sw"])}, rid=k)
        if k == "yi":
            r.open = k
    return r


def notifications(raw):
    N = raw["scrapes"]["notifications"]
    tones = {"var(--need)": "need", "rgb(111, 214, 151)": "ok", "rgb(133, 136, 145)": "faint", "rgb(143, 180, 255)": "accent"}
    r = Rows()
    for n in N["items"]:
        cells = {"title": t(n["title"]), "age": t(n["age"]), "text": t(n["text"]), "kind": dot({"need": "need", "ok": "done", "faint": "idle"}.get(tones.get(n["dot"]), "idle"))}
        if n["buttons"]:
            cells["actions"] = tags([b["text"] for b in n["buttons"]])
        r.add(n["title"], cells, dim=n["old"])
    return r


def sidebar(raw):
    G, LABM = raw["data"]["G"], raw["data"]["LABM"]
    r, group, root = Rows(), None, None
    for g in G:
        if g[0] == "lab":
            group = g[1]
            if g[1] in LABM:
                r.notes[g[1]] = LABM[g[1]]
            continue
        kind, state, title, ago = g[:4]
        cells = {"title": t(title), "state": dot(state), "age": t(ago)}
        rid = r.add(title, cells, group=group, parent=root if kind in ("mid", "last") else None)
        root = rid if kind == "root+" else root
        if len(g) > 4 and g[4]:
            r.selected = rid
    return r


def build(raw):
    """slug -> Rows for every list pane."""
    out = {"agents": agents(raw), "trace": trace(raw), "logs": logs(raw), "findings": findings(raw), "findings-table": findings_table(raw),
           "assets": assets(raw), "containers": containers(raw), "vms": vms(raw), "daemons": daemons(raw), "lsp": lsp(raw),
           "problems": problems(raw), "plugins": plugins(raw), "settings": settings(raw), "themes": themes(raw),
           "notifications": notifications(raw), "sidebar": sidebar(raw)}
    out.update(debugger(raw))
    return out
