"""Judge replay on recorded transcripts: no model, no network, no paid run.

The first Yi fixture is one real faux session driven over `yi acp` (prompt, prompt, `_yi/rewind`
to the second prompt, prompt), so its third prompt sits after the abandoned reply in the file.
The second is two real `yi ask --confirm` runs, the second `--continue`d, whose refused edit
before any read made Yi's router write its plan nudge; its prompt slots and cwd are scrubbed.
The Claude Code fixtures are three real transcripts and one subagent file with every text
replaced and the structure kept; the entries read as typed by hand carry `typed <line>: ...`.
"""
import contextlib, io, json, os, pathlib, re, shutil, sys, tempfile, types, unittest, urllib.error
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import judge_replay as jr  # noqa: E402

YI = jr.FIXTURES["yi"] / "1790469866572_01a0e052-044b-7923-bfb3-8e04284fa653.jsonl"
NUDGED = jr.FIXTURES["yi"] / "1790472350067_01a0e077-e929-7cf4-b1e3-58550db33230.jsonl"
CLAUDE = jr.FIXTURES["claude"]
RESUMED = CLAUDE / "-work-yi" / "76e146c7-2b99-4d7a-bbaf-e4f129f0328b.jsonl"
REMINDER = "<system-reminder>\nscrubbed reminder\n</system-reminder>\n"


def completion(content, cost=0.25):
    """A chat completion in OpenRouter's shape, `usage` as `usage: {include: true}` returns it."""
    return {"model": "z-ai/glm-5.3-flash-20260901", "choices": [{"message": {"role": "assistant", "content": content}}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 30, "cost": cost,
                      "prompt_tokens_details": {"cached_tokens": 64}}}


def boundary(texts):
    intent = [{"file": "f", "entry": f"e{i}", "text": t, "offset": 0} for i, t in enumerate(texts)]
    return {"id": "0", "intent": intent, "turn": {"entry": "t", "text": "done"}, "next": {"text": "NEXT-NEEDLE"},
            "reply": "REPLY-NEEDLE"}


# Scrubbed words that name the line they sit on; every tool result reads the same, so a copy
# renames each after its line.
NEEDLE = re.compile(r"(?:assistant text|command|description|file_path|typed|result at line) \d+\b")


def needled(scratch, pad=""):
    """The Claude fixtures copied under `scratch` with each tool result renamed after its line."""
    for source in jr.corpus_files(CLAUDE):
        copy = pathlib.Path(scratch) / source.relative_to(CLAUDE)
        copy.parent.mkdir(parents=True, exist_ok=True)
        copy.write_text("\n".join(line.replace('"scrubbed result"', f'"result at line {i}{pad}"')
                                  for i, line in enumerate(source.read_text().split("\n"))))
    return pathlib.Path(scratch)


def fixture_rows(root=CLAUDE):
    """(row, nodes) for every boundary in the Yi fixtures and the Claude ones under `root`."""
    rows = jr.extract_corpora([("yi", jr.FIXTURES["yi"]), ("claude", root)])
    return [(row, jr.READERS[row["corpus"]](pathlib.Path(row["session"]))) for row in rows]


class Readers(unittest.TestCase):
    def test_a_rewound_prompt_pairs_with_its_tree_ancestor_not_the_previous_line(self):
        rows = {r["next"]["text"]: r for r in jr.boundaries("yi", YI, jr.read_yi(YI))}
        rewound = rows["no: rename the parser first"]
        self.assertEqual(rewound["turn"]["text"], "I wrote the parser in parse.rs.")
        self.assertEqual([m["text"] for m in rewound["intent"]], ["write the parser"])
        self.assertEqual(rewound["reply"], "Renamed the parser to lex.")

    def test_a_child_session_is_not_the_owner(self):
        with tempfile.TemporaryDirectory() as scratch:
            child = pathlib.Path(scratch) / "rlm-1" / "sub-a" / YI.name
            child.parent.mkdir(parents=True)
            shutil.copy(YI, child)
            self.assertEqual(jr.extract_corpora([("yi", scratch)]), [])

    def test_a_host_nudge_is_not_the_owner(self):
        rows = jr.boundaries("yi", NUDGED, jr.read_yi(NUDGED))
        self.assertEqual([[m["text"] for m in r["intent"] + [r["next"]]] for r in rows],
                         [["fix the typo in notes.txt", "no: read it first"]], "the nudge read as the owner")
        lines = jr.prefix(rows[0], jr.read_yi(NUDGED)).split("\n")
        self.assertEqual([l for l in lines if l.startswith("[")],
                         ["[u1] fix the typo in notes.txt", "[call edit]", "[result edit]",
                          "[agent] The edit to notes.txt was refused."], "the nudge mid-turn cut the edit out")
        self.assertIn("path: notes.txt", lines)
        self.assertFalse([l for l in lines if "outgrown" in l], "a host nudge rendered as the conversation")

    def test_a_line_separator_inside_a_message_keeps_the_entry(self):
        # Claude Code and serde_json both write U+2028 raw inside a string; 13 typed messages in
        # the real corpus carried one, and `splitlines` cut each of those entries in two.
        for kind, source, words in (("yi", YI, "write the parser"), ("claude", RESUMED, "typed 65: the owner's words")):
            with self.subTest(kind), tempfile.TemporaryDirectory() as scratch:
                pasted = words.replace(" ", "\u2028", 1)
                (pathlib.Path(scratch) / source.name).write_text(source.read_text().replace(words, pasted))
                rows = jr.extract_corpora([(kind, scratch)])
                self.assertIn(pasted, {m["text"] for r in rows for m in r["intent"] + [r["next"]]})

    def test_a_resumed_transcript_is_one_conversation(self):
        # Claude Code resumes into a new file that rewrites the old entries under the new
        # sessionId and goes on (a real pair did: 9db2cd1e, copied into 7d9b1258).
        entries = [json.loads(line) for line in RESUMED.read_text().split("\n") if line.strip()]
        end = next(e for e in entries if e.get("uuid", "").startswith("bff7057e"))
        typed = next(e for e in entries if e.get("uuid", "").startswith("7950055d"))
        name = next(f"{n:08x}-resumed.jsonl" for n in range(99)
                    if jr.split_of("claude", f"{n:08x}-resumed.jsonl") != jr.split_of("claude", RESUMED.name))
        leaf = next(e["uuid"] for e in reversed(entries) if e.get("uuid"))
        reply = {**end, "uuid": "resumed-reply", "parentUuid": leaf,
                 "message": {**end["message"], "id": "resumed", "content": [{"type": "text", "text": "resumed text"}]}}
        again = {**typed, "uuid": "resumed-typed", "parentUuid": "resumed-reply",
                 "message": {**typed["message"], "content": "typed resumed: go on"}}
        copy = [{**e, "sessionId": name[:-6]} if "sessionId" in e else e for e in entries] + [reply, again]
        with tempfile.TemporaryDirectory() as scratch:
            shutil.copy(RESUMED, scratch)
            (pathlib.Path(scratch) / name).write_text("".join(json.dumps(e) + "\n" for e in copy))
            rows = jr.extract_corpora([("claude", scratch)])
        self.assertEqual(sorted(r["next"]["text"] for r in rows), ["typed 65: the owner's words", "typed resumed: go on"])
        self.assertEqual(len({r["split"] for r in rows}), 1, "one conversation landed on both sides")

    def test_only_typed_messages_count_in_claude_transcripts(self):
        typed = {m for p in jr.corpus_files(CLAUDE) if "subagents" not in p.parts
                 for m in re.findall(r"typed \d+: [^\"]+", p.read_text())}
        rows = jr.extract_corpora([("claude", CLAUDE)])
        seen = {m["text"] for r in rows for m in r["intent"] + [r["next"]]}
        self.assertEqual(seen, typed, "a harness entry was read as the owner, or a typed one was lost")
        self.assertEqual(len(typed), 12)
        notice = {"type": "user", "origin": {"kind": "task-notification"}, "message": {"content": "plain words"}}
        self.assertIsNone(jr.claude_human(notice), "the harness's own origin marker names it not human")
        self.assertFalse([r for r in rows if "subagents" in r["session"]])
        after = next(r for r in rows if r["next"]["text"].startswith("typed 339:"))
        self.assertEqual([m["text"][:9] for m in after["intent"]], ["typed 5: ", "typed 273"],
                         "the intent record stops at the compaction")

    def test_a_citation_reads_the_owners_words_at_its_bytes_in_the_file(self):
        rows = jr.extract_corpora([("claude", CLAUDE)])
        first = next(r for r in rows if r["next"]["text"].startswith("typed 73:"))
        message = first["intent"][0]
        self.assertEqual(message["offset"], len(REMINDER.encode()))
        cited = jr.cite(first, {"msg": "u1", "quote": "the owner's   words"})
        entry = next(json.loads(line) for line in pathlib.Path(message["file"]).read_text().splitlines()
                     if json.loads(line).get("uuid") == message["entry"])
        stored = jr.blocks_text(entry["message"]["content"])
        self.assertEqual(stored.encode()[slice(*cited["bytes"])], b"the owner's words")
        self.assertEqual(cited["entry"], message["entry"])


class Prefix(unittest.TestCase):
    def test_the_prefix_holds_nothing_at_or_after_the_boundary(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = needled(scratch)
            copy = root / "-work-yi" / RESUMED.name
            entries = [json.loads(line) for line in copy.read_text().split("\n") if line.strip()]
            parallel = entries[50]
            # A result for a call on the path, on a branch of its own after the owner's message:
            # a reader that trusts the call id over file order shows it.
            late = {"type": "user", "uuid": "late-result", "parentUuid": parallel["uuid"],
                    "message": {"role": "user", "content": [{"type": "tool_result", "content": "LATE RESULT",
                                                             "tool_use_id": parallel["message"]["content"][0]["id"]}]}}
            # A rewound branch off the turn end, earlier in the file than the boundary: the owner's
            # abandoned message, the call it drew and that call's result.
            end = next(e for e in entries if e.get("uuid", "").startswith("bff7057e"))
            rewound = [{"type": "user", "uuid": "gone-typed", "parentUuid": end["uuid"],
                        "message": {"role": "user", "content": "typed gone: REWOUND WORDS"}},
                       {"type": "assistant", "uuid": "gone-call", "parentUuid": "gone-typed", "message": {
                           "id": "gone", "role": "assistant", "stop_reason": "tool_use", "content": [
                               {"type": "tool_use", "id": "toolu_gone", "name": "Bash", "input": {"command": "REWOUND CALL"}}]}},
                       {"type": "user", "uuid": "gone-result", "parentUuid": "gone-call", "message": {"role": "user", "content": [
                           {"type": "tool_result", "tool_use_id": "toolu_gone", "content": "REWOUND RESULT"}]}}]
            lines = copy.read_text().split("\n")
            at = next(i for i, line in enumerate(lines) if f'"uuid": "{end["uuid"]}"' in line) + 1
            copy.write_text("\n".join(lines[:at] + [json.dumps(e) for e in rewound] + lines[at:]) + json.dumps(late) + "\n")
            checked, shown_by_next = 0, {}
            for row, nodes in fixture_rows(root):
                shown = jr.prefix(row, nodes)
                shown_by_next[row["next"]["text"]] = shown
                at = nodes[row["next"]["entry"]]["order"]
                words = lambda n: [n.get("text") or "", *(r["text"] for r in n.get("results") or []),
                                   *(v for c in n.get("calls") or [] for v in (c["args"] or {}).values() if isinstance(v, str))]
                earlier = {w for n in nodes.values() if n["order"] < at for w in words(n)}
                for later in {w for n in nodes.values() if n["order"] >= at for w in words(n)} - earlier - {""}:
                    checked += 1
                    self.assertIsNone(re.search(re.escape(later) + r"(?!\d)", shown), f"{later!r} leaked before its boundary")
        self.assertGreater(checked, 100)
        self.assertNotIn("LATE RESULT", shown_by_next["typed 65: the owner's words"])
        for rewound in ("REWOUND WORDS", "REWOUND CALL", "REWOUND RESULT"):
            self.assertNotIn(rewound, shown_by_next["typed 65: the owner's words"], "the rewound branch leaked")
        for twig in ("description 51", *(f"result at line {i}" for i in range(52, 58))):
            self.assertRegex(shown_by_next["typed 65: the owner's words"], re.escape(twig) + r"(?!\d)",
                             "a parallel call or its result hanging off the path was lost")
        for abandoned in ("now add the tests", "Tests added in tests/parse.rs."):
            self.assertNotIn(abandoned, shown_by_next["no: rename the parser first"], "the abandoned branch leaked")

    def test_a_block_rewritten_further_down_under_the_same_uuids_changes_no_prefix(self):
        # Claude Code rewrote lines 6-1255 of a real transcript at 2236-3308 under the same uuids;
        # placed by the copy, 29 later boundaries lost earlier owner messages and no row said so.
        source = CLAUDE / "-work-yi" / "466b740d-062b-4cef-8ab7-f101e5f7b06c.jsonl"
        lines = source.read_text().split("\n")
        with tempfile.TemporaryDirectory() as scratch:
            copy = pathlib.Path(scratch) / source.name
            copy.write_text("\n".join(lines + lines[:300]))
            prefixes = [{r["next"]["text"]: jr.prefix(r, nodes) for r in jr.boundaries("claude", path, nodes)}
                        for path in (source, copy) for nodes in [jr.read_claude(path)]]
        self.assertEqual(len(prefixes[0]), 6)
        self.assertEqual(prefixes[1], prefixes[0])

    def test_a_transcript_whose_path_changed_since_extract_is_an_error_row_not_a_call(self):
        # The corpus is live: once the path's owner messages move, [uN] no longer names the message
        # a citation resolves against, so the boundary is refused rather than asked.
        out, corpus = tempfile.mkdtemp(), tempfile.mkdtemp()
        for scratch in (out, corpus):
            self.addCleanup(shutil.rmtree, scratch, True)
        shutil.copytree(CLAUDE, corpus, dirs_exist_ok=True)
        changed = pathlib.Path(corpus) / "-work-yi" / "466b740d-062b-4cef-8ab7-f101e5f7b06c.jsonl"
        with contextlib.redirect_stdout(io.StringIO()):
            jr.main(["extract", "--out", out, "--corpus", f"claude:{corpus}"])
            jr.main(["label", "--out", out, "--model", "faux/faux-1", "--dry"])
            changed.write_text("\n".join(json.dumps({**json.loads(line), "isMeta": True}) if "typed 6: " in line else line
                                         for line in changed.read_text().split("\n")))
            with mock.patch.object(jr, "request", wraps=jr.request) as request:
                self.assertEqual(jr.main(["judge", "--out", out, "--model", "faux/faux-1", "--dry", "--arm", "judge"]), 0)
        rows = [r for p in (pathlib.Path(out) / "verdicts").glob("*.jsonl") for r in jr.read_rows(p)]
        stale = [r for r in rows if "changed since extract" in r.get("error", "")]
        self.assertTrue(stale)
        self.assertFalse([r for r in stale if "verdict" in r])
        self.assertEqual(request.call_count, len(rows) - len(stale))

    def test_the_tool_cap_speaks_at_limit_plus_one(self):
        nodes = jr.read_yi(YI)
        row = next(r for r in jr.boundaries("yi", YI, nodes) if r["next"]["text"] == "now add the tests")
        size = len("(no output)")
        at = jr.prefix(row, nodes, tool_chars=size).split("\n")
        self.assertEqual(at[at.index("[result bash]") + 1], "(no output)")
        self.assertFalse([l for l in at if l.startswith("[… ")])
        over = jr.prefix(row, nodes, tool_chars=size - 1).split("\n")
        at = over.index("[result bash]")
        self.assertEqual(over[at + 1:at + 3], ["(no output", f"[… kept {size - 1} of {size} chars; tool_chars={size - 1}]"])
        call = jr.prefix(row, nodes, tool_chars=1).split("\n")
        at = call.index("[call bash]")
        self.assertEqual(call[at + 1:at + 3], ["command: l", "[… kept 1 of 2 chars of `command`; tool_chars=1]"])

    def test_the_prefix_cap_cuts_oldest_results_first_at_limit_plus_one_and_keeps_the_owner(self):
        with tempfile.TemporaryDirectory() as scratch:
            row, nodes = next((r, n) for r, n in fixture_rows(needled(scratch)) if r["next"]["text"].startswith("typed 339:"))
            whole = jr.prefix(row, nodes, prefix_chars=10**9)
            limit, lines = len(whole), whole.split("\n")
            owners = [l for l in lines if l.startswith("[u")]
            results = [l for l in lines if l.startswith("result at line")]
            agent = sum(1 for l in lines if l.startswith(("[agent]", "[call ")))
            at = max(i for i, l in enumerate(lines) if l.startswith("[u"))
            turn = sum(1 for l in lines[at:] if l.startswith("result at line"))
            said = [l for l in lines[at:] if l.startswith(("[agent]", "[call "))]
            self.assertEqual(jr.prefix(row, nodes, prefix_chars=limit), whole)
            cut = jr.prefix(row, nodes, prefix_chars=limit - 1)
            self.assertLessEqual(len(cut), limit - 1)
            kept = [l for l in cut.split("\n") if l.startswith("result at line")]
            self.assertEqual(kept, results[len(results) - len(kept):], "a newer result went before an older one")
            self.assertEqual(cut.split("\n")[0], f"[… prefix_chars={limit - 1} kept {len(kept)} of {len(results)} tool "
                             f"results ({turn} of {turn} in the last turn), {agent} of {agent} agent texts and calls; "
                             'each "[… n cut: prefix_chars]" row is a gap]')
            gaps = [int(l.split()[1]) for l in cut.split("\n") if re.fullmatch(r"\[… \d+ cut: prefix_chars\]", l)]
            self.assertEqual(sum(gaps), len(results) - len(kept), "a cut left no row where it was")
            deep = jr.prefix(row, nodes, prefix_chars=len("\n".join(owners)) + 1).split("\n")
            self.assertEqual([l for l in deep if l.startswith("[u")], owners, "an owner's message was cut")
            self.assertEqual([l for l in deep if l.startswith(("[agent]", "[call "))], said,
                             "the last turn's agent text went, or an older one stayed")
            self.assertTrue(deep[0].startswith(f"[… prefix_chars={len(chr(10).join(owners)) + 1} kept 0 of {len(results)} "
                                               f"tool results (0 of {turn} in the last turn), {len(said)} of {agent} "
                                               "agent texts and calls;"), deep[0])
            alone = jr.prefix(row, nodes, prefix_chars=10, intent_chars=len(owners[-1]) + 1).split("\n")
            self.assertEqual([l for l in alone if l.startswith("[u")], owners[-1:])
            self.assertIn(f"[… kept the newest 1 of {len(owners)} messages; intent_chars={len(owners[-1]) + 1} "
                          f"cut u1..u{len(owners) - 1}]", alone)

    def test_the_prefix_cap_reaches_the_last_turns_results_last_oldest_first(self):
        # 228 of 666 capped real prefixes lost every result of the judged turn while older agent
        # text stayed. Results padded to a real size, so a gap row is cheap beside each one.
        with tempfile.TemporaryDirectory() as scratch:
            row, nodes = next((r, n) for r, n in fixture_rows(needled(scratch, " " + "x" * 400))
                              if r["next"]["text"].startswith("typed 339:"))
            items = jr.conversation(row, nodes, jr.TOOL_CHARS)
            whole = jr.prefix(row, nodes, prefix_chars=10**9)
            last = max(i for i, (kind, _) in enumerate(items) if kind == jr.OWNER)
            tiers = {}
            for i, (kind, lines) in enumerate(items):
                if kind != jr.OWNER:
                    text = "\n".join(lines)
                    tiers.setdefault((kind, i < last), []).append((NEEDLE.search(text).group(), len(text) + 1))
            size = lambda tier: sum(n for _, n in tiers[tier])
            old_results, old_agent, results, said = (jr.RESULT, True), (jr.AGENT, True), (jr.RESULT, False), (jr.AGENT, False)
            for cap, partial in ((len(whole) - size(old_results) - size(old_agent) // 2, old_agent),
                                 (len(whole) - size(old_results) - size(old_agent) - size(results) // 2, results)):
                cut = jr.prefix(row, nodes, prefix_chars=cap)
                names = {tier: [n for n, _ in tiers[tier]] for tier in tiers}
                kept = {tier: [n for n in names[tier] if re.search(re.escape(n) + r"(?!\d)", cut)] for tier in tiers}
                for tier in (old_results, results):
                    self.assertEqual(kept[tier], names[tier][len(names[tier]) - len(kept[tier]):], f"{tier} not oldest first")
                for earlier, later in ((old_results, old_agent), (old_agent, results)):
                    if kept[later] != names[later]:
                        self.assertEqual(kept[earlier], [], f"{later} was cut while {earlier} stayed")
                self.assertEqual(kept[said], names[said], "the last turn's agent text or a call was cut")
                self.assertTrue(0 < len(kept[partial]) < len(names[partial]), (cap, partial, len(kept[partial])))
                self.assertLessEqual(len(cut), cap)
                count = lambda kind: [sum(len(t[tier]) for tier in tiers if tier[0] == kind) for t in (kept, names)]
                self.assertEqual(cut.split("\n")[0], "[… prefix_chars={} kept {} of {} tool results ({} of {} in the last turn), "
                                 "{} of {} agent texts and calls; each \"[… n cut: prefix_chars]\" row is a gap]".format(
                                     cap, *count(jr.RESULT), len(kept[results]), len(names[results]), *count(jr.AGENT)))

    def test_quotes_resolve_to_utf8_bytes_and_paraphrases_do_not(self):
        text = "naïve café — ok\n  then   more"
        self.assertEqual(jr.resolve(text, "café — ok"), [7, 19])
        self.assertEqual(jr.resolve(text, "ok then more"), [17, 33])
        self.assertIsNone(jr.resolve(text, "a naive cafe"))
        self.assertIsNone(jr.resolve(text, ""))
        self.assertIsNone(jr.cite(boundary(["x"]), {"msg": "u2", "quote": "x"})["bytes"])

    def test_the_labellers_intent_cap_speaks_at_limit_plus_one(self):
        limit = len("[u1] " + "a" * 10) + 1 + len("[u2] " + "b" * 20) + 1
        self.assertEqual(jr.owner_rows(boundary(["a" * 10, "b" * 20])["intent"], cap=limit),
                         ["[u1] " + "a" * 10, "[u2] " + "b" * 20])
        self.assertEqual(jr.owner_rows(boundary(["a" * 11, "b" * 20])["intent"], cap=limit),
                         [f"[… kept the newest 1 of 2 messages; intent_chars={limit} cut u1..u1]", "[u2] " + "b" * 20])
        self.assertIn("\n[u1] the words\n", jr.render_label(boundary(["the words"])))
        # 51 real boundaries held more than 60k chars of owner text; the label reads 240k of it.
        old, cap = "[u1] the old words", jr.LABEL_INTENT_CHARS
        fill = cap - (len(old) + 1) - len("[u2] ") - 1
        at = jr.render_label(boundary(["the old words", "b" * fill])).split("\n")
        self.assertEqual(at[1], old, "an owner's message inside the label cap was cut")
        over = jr.render_label(boundary(["the old words", "b" * (fill + 1)])).split("\n")
        self.assertEqual(over[1], f"[… kept the newest 1 of 2 messages; intent_chars={cap} cut u1..u1]")
        self.assertNotIn(old, over)


class Arms(unittest.TestCase):
    def test_the_self_arm_ends_on_the_owners_check_and_the_judge_arm_does_not(self):
        check = (jr.REPLAY / "check.md").read_text().strip()
        out = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, out, True)
        sent = {}
        with contextlib.redirect_stdout(io.StringIO()):
            jr.main(["extract", "--out", out, "--corpus", f"yi:{jr.FIXTURES['yi']}", "--corpus", f"claude:{CLAUDE}"])
            jr.main(["label", "--out", out, "--model", "faux/faux-1", "--dry"])
            for arm in jr.ARMS:
                with mock.patch.object(jr, "request", wraps=jr.request) as request:
                    self.assertEqual(jr.main(["judge", "--out", out, "--model", "faux/faux-1", "--dry", "--arm", arm]), 0)
                sent[arm] = [(c.args[2], c.args[3]) for c in request.call_args_list]
        self.assertTrue(sent["self"])
        self.assertEqual([turns[-1] for _, turns in sent["self"]], [check] * len(sent["self"]))
        self.assertTrue(all(len(turns) == 1 and check not in turns[0] and check not in system
                            for system, turns in sent["judge"]))
        self.assertEqual(sorted(turns[0] for _, turns in sent["self"]), sorted(turns[0] for _, turns in sent["judge"]),
                         "the arms read different prefixes")
        files = {p.name: {r.get("arm") for r in jr.read_rows(p)} for p in (pathlib.Path(out) / "verdicts").glob("*.jsonl")}
        self.assertEqual(sorted(files.values(), key=str), [{"judge"}, {"self"}])
        self.assertTrue(all(f"-{arms.copy().pop()}-" in name for name, arms in files.items()), files)


class Labels(unittest.TestCase):
    def test_an_intent_loss_whose_rests_on_does_not_resolve_is_demoted_to_new_info(self):
        out = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, out, True)
        expected, turn = [], iter(range(10**6))

        def reply(body):
            owner = re.search(r"^\[u1\] (\S+ \S+)", body["messages"][-1]["content"], re.M)
            mode = next(turn) % 3
            quote = owner.group(1) if owner and mode == 0 else "words the owner never wrote"
            expected.append(bool(owner) and mode == 0)
            return completion(json.dumps({"label": "objected", "kind": "intent_loss", "objection": "o", "quote": "q",
                                          "rests_on": [{"msg": "u99" if mode == 2 else "u1", "quote": quote},
                                                       {"msg": "u98", "quote": quote}]}), cost=0.001)

        with contextlib.redirect_stdout(io.StringIO()), mock.patch.dict(os.environ, {jr.KEY: "k"}), \
                mock.patch.object(jr, "post", side_effect=reply):
            jr.main(["extract", "--out", out, "--corpus", f"yi:{jr.FIXTURES['yi']}", "--corpus", f"claude:{CLAUDE}"])
            self.assertEqual(jr.main(["label", "--out", out, "--model", Call.MODEL, "--cap-usd", "1"]), 0)
        rows = jr.read_rows(pathlib.Path(out) / "labels-v2.jsonl")
        self.assertEqual({True, False}, set(expected))
        self.assertEqual([(r["kind"], r["demoted"]) for r in rows],
                         [("intent_loss", False) if ok else ("new_info", True) for ok in expected])
        self.assertFalse((pathlib.Path(out) / "labels.jsonl").exists(), "v1's labels were written")
        classes, rests, source = jr.effective_labels(pathlib.Path(out))
        self.assertEqual((source, sorted(set(classes.values()))), ("labels-v2.jsonl", ["intent_loss", "new_info"]))
        self.assertTrue(all(rests[r["id"]] == {"u1"} for r in rows if r["kind"] == "intent_loss"))


class Call(unittest.TestCase):
    MODEL = "openrouter/z-ai/glm-5.3-flash"

    def args(self, effort=None, **extra):
        return types.SimpleNamespace(model=self.MODEL, dry=False, effort=effort, **extra)

    def test_the_request_is_the_prompt_and_the_input_with_no_tools(self):
        for row, nodes in fixture_rows():
            shown = jr.prefix(row, nodes)
            body = jr.request(self.MODEL, "judge", "SYSTEM", [shown, "CHECK"])
            self.assertEqual(sorted(body), ["messages", "model", "response_format", "temperature", "usage"])
            self.assertEqual((body["model"], body["temperature"], body["usage"]), ("z-ai/glm-5.3-flash", 0, {"include": True}))
            self.assertEqual(body["messages"], [{"role": "system", "content": "SYSTEM"}, {"role": "user", "content": shown},
                                                {"role": "user", "content": "CHECK"}])
        for phase, name in jr.SCHEMAS.items():
            schema = json.loads((jr.REPLAY / f"{name}.schema.json").read_text())
            self.assertEqual(jr.request(self.MODEL, phase, "s", ["p"])["response_format"],
                             {"type": "json_schema", "json_schema": {"name": name, "strict": True, "schema": schema}})
        for effort in (None, "high"):
            with self.subTest(effort=effort), mock.patch.object(jr, "post", return_value=completion("{}")) as post:
                jr.ask(self.args(effort=effort), "judge", "s", ["p"])
                sent = post.call_args.args[0]
                if effort:
                    self.assertEqual(sent["reasoning"]["effort"], "high")
                else:
                    self.assertNotIn("reasoning", sent)
        self.assertEqual(jr.run_name(self.MODEL, "self", "frame"), f"openrouter_z-ai_glm-5.3-flash-self-{jr.sha('frame')[:12]}")
        self.assertEqual(jr.run_name(self.MODEL, "self", "frame", "high"), jr.run_name(self.MODEL, "self", "frame") + "-high")

    def test_a_reply_parses_strict_or_fenced_and_garbage_is_an_error_row(self):
        answer = {"label": "objected", "objection": "wrong file", "quote": "no, the other one"}
        for content, parsed in ((json.dumps(answer), answer), (f"```json\n{json.dumps(answer)}\n```", answer),
                                ("It was accepted, I think.", None), ("{\"label\": \"objected\",", None)):
            with self.subTest(content), mock.patch.object(jr, "post", return_value=completion(content)):
                row = jr.ask(self.args(), "label", "s", ["p"])
                self.assertEqual(row.get("answer"), parsed)
                if parsed is None:
                    self.assertEqual((row["error"], row["content"]), ("no JSON object in the reply", content))
                self.assertEqual((row["providerModel"], row["input"], row["output"], row["cached"], row["costUsd"]),
                                 ("z-ai/glm-5.3-flash-20260901", 120, 30, 64, 0.25))

    def test_a_real_run_refuses_a_model_off_openrouter_and_a_missing_key(self):
        out = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, out, True)
        for model, environ, needle in (("anthropic/claude-x", {jr.KEY: "k"}, "--model is openrouter/<id>, not anthropic/claude-x"),
                                       (self.MODEL, {}, f"{jr.KEY} is unset"), (self.MODEL, {jr.KEY: " "}, f"{jr.KEY} is unset")):
            err = io.StringIO()
            with self.subTest(model=model, environ=environ), mock.patch.dict(os.environ, environ, clear=True), \
                    mock.patch.object(jr, "post") as post, contextlib.redirect_stderr(err):
                self.assertEqual(jr.main(["label", "--out", out, "--model", model, "--cap-usd", "1"]), 2)
                self.assertIn(needle, err.getvalue())
                post.assert_not_called()

    def test_the_cap_stops_dispatch_once_the_summed_cost_reaches_it(self):
        items = [{"id": f"b{i}"} for i in range(5)]
        answer = json.dumps({"label": "accepted", "objection": "", "quote": ""})
        # A reply with no usage.cost leaves the cap blind, so it stops the phase after one call.
        for cap, cost, sent, exit in ((0.5, 0.25, 2, 0), (0.5 + 1e-9, 0.25, 3, 0), (0.5, None, 1, 1)):
            with self.subTest(cap=cap, cost=cost), tempfile.TemporaryDirectory() as scratch, \
                    contextlib.redirect_stdout(io.StringIO()), \
                    mock.patch.object(jr, "post", return_value=completion(answer, cost=cost)) as post:
                sink = pathlib.Path(scratch) / "labels.jsonl"
                done = jr.run_calls(self.args(jobs=1, cap_usd=cap), "label", items, sink,
                                    lambda item: ("s", "p", None, dict(item)), lambda row: row)
                self.assertEqual((done, post.call_count, len(jr.read_rows(sink))), (exit, sent, sent))

    def test_a_p_objection_outside_zero_to_one_is_a_call_without_a_verdict(self):
        # A model answering in percent would outrank every answer on the 0-1 scale and bend the
        # AUROC; the row is refused instead, and the gate counts it.
        out = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, out, True)
        said = iter([0.8, 80, True, None])

        def reply(body):
            chance = next(said)
            answer = {"verdict": "revise", "objections": []} | ({} if chance is None else {"p_objection": chance})
            return completion(json.dumps(answer), cost=0.001)

        with contextlib.redirect_stdout(io.StringIO()), mock.patch.dict(os.environ, {jr.KEY: "k"}), \
                mock.patch.object(jr, "post", side_effect=reply):
            jr.main(["extract", "--out", out, "--corpus", f"yi:{jr.FIXTURES['yi']}", "--corpus", f"claude:{CLAUDE}"])
            jr.main(["label", "--out", out, "--model", "faux/faux-1", "--dry"])
            self.assertEqual(jr.main(["judge", "--out", out, "--model", self.MODEL, "--arm", "judge", "--limit", "4",
                                      "--cap-usd", "1"]), 0)
        rows = [r for p in (pathlib.Path(out) / "verdicts").glob("*.jsonl") for r in jr.read_rows(p)]
        self.assertEqual([(r.get("p_objection"), r.get("error")) for r in rows],
                         [(0.8, None)] + [(None, "no verdict with a p_objection in [0, 1]")] * 3)

    def test_retries_are_bounded_and_a_client_error_is_not_retried(self):
        for code, attempts in ((429, jr.RETRIES + 1), (503, jr.RETRIES + 1), (400, 1)):
            error = urllib.error.HTTPError(jr.OPENROUTER, code, "status", {}, io.BytesIO(b"{}"))
            self.addCleanup(error.close)
            with self.subTest(code=code), mock.patch.object(jr.urllib.request, "urlopen", side_effect=error) as urlopen, \
                    mock.patch.object(jr.time, "sleep"):
                with self.assertRaises(urllib.error.HTTPError):
                    jr.post({})
                self.assertEqual(urlopen.call_count, attempts)


class Metrics(unittest.TestCase):
    def test_balanced_accuracy_on_a_hand_table(self):
        pairs = [(True, True)] * 3 + [(True, False)] + [(False, False)] * 2 + [(False, True)] * 2
        self.assertEqual(jr.balanced(pairs), (0.625, 0.75, 0.5))
        self.assertIsNone(jr.balanced([(True, True)]), "specificity is undefined with no negatives")
        self.assertEqual(jr.balanced([(True, False), (False, False)])[0], 0.5)
        self.assertEqual(jr.balanced([(True, True), (False, True)])[0], 0.5)

    def test_auroc_on_a_hand_table_with_ties(self):
        # The 0.9 positive beats both negatives; each 0.5 positive ties the 0.5 negative (a half)
        # and beats the 0.1: (2 + 1.5 + 1.5) / (3 * 2).
        self.assertAlmostEqual(jr.auroc([(True, 0.9), (True, 0.5), (True, 0.5), (False, 0.5), (False, 0.1)]), 5 / 6)
        self.assertEqual(jr.auroc([(True, 0.3), (False, 0.3)]), 0.5)
        self.assertEqual(jr.auroc([(True, 0.1), (False, 0.9)]), 0.0)
        self.assertIsNone(jr.auroc([(True, 0.9)]), "AUROC is undefined with no negatives")

    def test_the_bootstrap_resamples_sessions(self):
        # A session resample is s1+s1 (1.0), s2+s2 (0.0) or s1+s2 (0.5); resampling the 40 rows
        # instead would pile up near 0.5 and hide how much one session decides.
        by_session = {"s1": [(True, True)] * 10 + [(False, False)] * 10,
                      "s2": [(True, False)] * 10 + [(False, True)] * 10}
        accuracy = lambda pairs: (jr.balanced(pairs) or (None,))[0]
        self.assertEqual(accuracy(by_session["s1"] + by_session["s2"]), 0.5)
        self.assertEqual(jr.bootstrap(by_session, accuracy), (0.0, 1.0))
        self.assertIsNone(jr.bootstrap({"s1": [(True, True)]}, accuracy))
        chances = {"s1": [(True, 0.9)] * 10 + [(False, 0.1)] * 10, "s2": [(True, 0.1)] * 10 + [(False, 0.9)] * 10}
        self.assertEqual(jr.auroc(chances["s1"] + chances["s2"]), 0.5)
        self.assertEqual(jr.bootstrap(chances, jr.auroc), (0.0, 1.0))

    def test_catch_needs_a_flag_citing_what_the_label_rests_on(self):
        def verdict(key, said, cited, resolved=True):
            return {"id": key, "split": "fit", "verdict": said, "p_objection": 0.5, "objections": [
                {"text": "t", "citations": [{"msg": m, "bytes": [0, 1] if resolved else None} for m in cited]}]}

        classes = {"meets": "intent_loss", "misses": "intent_loss", "accepts": "intent_loss", "unresolved": "intent_loss",
                   "revealed": "check_revealed", "fine": "accepted"}
        rows = [verdict("meets", "revise", ["u2", "u1"]), verdict("misses", "revise", ["u2"]),
                verdict("accepts", "accept", ["u1"]), verdict("unresolved", "escalate", ["u1"], resolved=False),
                verdict("revealed", "revise", []), verdict("fine", "accept", [])]
        scored = jr.section(rows, classes, {k: {"u1"} for k in classes}, {})
        self.assertEqual((scored["catch"], scored["revealed"]), (1 / 4, 1.0))

    def test_an_interleaved_sample_cut_at_k_stays_balanced(self):
        # Ids in hash order front-load negatives, as the v1 sample did: the first 25 are accepted.
        rows = [{"id": f"{i:04x}", "split": "fit"} for i in range(60)]
        classes = {r["id"]: "accepted" if i < 25 or i % 5 == 0 else "intent_loss" for i, r in enumerate(rows)}
        classes["0021"], classes["0022"] = "check_revealed", "new_info"
        positives = sum(1 for c in classes.values() if c in jr.POSITIVE)
        negatives = sum(1 for c in classes.values() if c == "accepted")
        for k in (1, 2, 3, 9, 2 * min(positives, negatives)):
            cut = jr.select(rows, None, k, classes)
            taken = sum(1 for r in cut if classes[r["id"]] in jr.POSITIVE)
            self.assertEqual(len(cut), k)
            self.assertLessEqual(abs(taken - (k - taken)), 1, k)
        self.assertNotIn("0022", {r["id"] for r in jr.select(rows, None, None, classes)}, "new_info was sampled")

    def test_the_gate_line_at_its_thresholds(self):
        held = {"resolution": 0.95, "auroc_interval": (0.501, 0.9), "missing": 0}
        self.assertTrue(jr.gate_line(held).startswith("gate: PASS"))
        for change in ({"auroc_interval": (0.5, 0.9)}, {"resolution": 0.949}, {"resolution": None, "auroc_interval": None},
                       {"missing": 1}):
            self.assertTrue(jr.gate_line({**held, **change}).startswith("gate: FAIL"), change)

    def test_the_dry_pipeline_watches_a_tilde_corpus_and_refuses_an_empty_one(self):
        # `--corpus yi:~/.yi/sessions` is the docstring's own form; the read-only proof listed it
        # unexpanded, found nothing on either side, and printed ok having checked nothing.
        with tempfile.TemporaryDirectory() as home, mock.patch.dict(os.environ, {"HOME": home}):
            shutil.copytree(jr.FIXTURES["yi"], pathlib.Path(home) / "sessions")
            touched = pathlib.Path(home) / "sessions" / YI.name

            def report(_args):
                touched.write_text(touched.read_text() + "\n")
                return 0

            with mock.patch.object(jr, "cmd_report", report), contextlib.redirect_stdout(io.StringIO()) as said:
                self.assertEqual(jr.main(["all", "--dry", "--model", "faux/faux-1", "--corpus", "yi:~/sessions"]), 1)
            self.assertIn("FAIL judge_replay_dry: a file under the corpus changed", said.getvalue())
            (pathlib.Path(home) / "empty").mkdir()
            with contextlib.redirect_stdout(io.StringIO()) as said:
                self.assertEqual(jr.main(["all", "--dry", "--model", "faux/faux-1", "--corpus", "yi:~/empty"]), 1)
            self.assertIn("FAIL judge_replay_dry: no file under the corpus", said.getvalue())

    def test_the_split_sends_about_seventy_percent_to_fit(self):
        fit = sum(jr.split_of("claude", f"{n}.jsonl") == "fit" for n in range(2000))
        self.assertTrue(1300 <= fit <= 1500, fit)


if __name__ == "__main__":
    unittest.main()
