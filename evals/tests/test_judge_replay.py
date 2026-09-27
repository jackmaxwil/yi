"""Judge replay on recorded transcripts: no model, no network, no paid run.

The first Yi fixture is one real faux session driven over `yi acp` (prompt, prompt, `_yi/rewind`
to the second prompt, prompt), so its third prompt sits after the abandoned reply in the file.
The second is two real `yi ask --confirm` runs, the second `--continue`d, whose refused edit
before any read made Yi's router write its plan nudge; its prompt slots and cwd are scrubbed.
The Claude Code fixtures are three real transcripts and one subagent file with every text
replaced and the structure kept; the entries read as typed by hand carry `typed <line>: ...`.
"""
import json, pathlib, re, shutil, sys, tempfile, threading, types, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import judge_replay as jr  # noqa: E402

YI = jr.FIXTURES["yi"] / "1790469866572_01a0e052-044b-7923-bfb3-8e04284fa653.jsonl"
NUDGED = jr.FIXTURES["yi"] / "1790472350067_01a0e077-e929-7cf4-b1e3-58550db33230.jsonl"
CLAUDE = jr.FIXTURES["claude"]
RESUMED = CLAUDE / "-work-yi" / "76e146c7-2b99-4d7a-bbaf-e4f129f0328b.jsonl"
REMINDER = "<system-reminder>\nscrubbed reminder\n</system-reminder>\n"


def boundary(texts, calls=(), final="done"):
    intent = [{"file": "f", "entry": f"e{i}", "text": t, "offset": 0} for i, t in enumerate(texts)]
    return {"id": "0", "intent": intent, "turn": {"entry": "t", "text": final, "calls": list(calls)},
            "next": {"text": "NEXT-NEEDLE"}, "reply": "REPLY-NEEDLE"}


class Readers(unittest.TestCase):
    def test_a_rewound_prompt_pairs_with_its_tree_ancestor_not_the_previous_line(self):
        rows = {r["next"]["text"]: r for r in jr.boundaries("yi", YI, jr.read_yi(YI))}
        rewound = rows["no: rename the parser first"]
        self.assertEqual(rewound["turn"]["text"], "I wrote the parser in parse.rs.")
        self.assertEqual([m["text"] for m in rewound["intent"]], ["write the parser"])
        self.assertEqual(rewound["reply"], "Renamed the parser to lex.")
        self.assertEqual(rows["now add the tests"]["turn"]["calls"], [{"name": "bash", "head": "ls"}])

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
        self.assertEqual(rows[0]["turn"]["calls"], [{"name": "edit", "head": "notes.txt"}],
                         "the nudge mid-turn cut the edit out of the turn")

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


class Judge(unittest.TestCase):
    def test_the_judge_sees_nothing_after_the_boundary(self):
        rows = jr.extract_corpora([("yi", jr.FIXTURES["yi"]), ("claude", CLAUDE)])
        self.assertGreater(len(rows), 5)
        for row in rows:
            shown = jr.render_judge(row)
            self.assertNotIn(row["next"]["text"], shown)
            if row["reply"]:
                self.assertNotIn(row["reply"], shown)
        rewound = next(r for r in rows if r["next"]["text"] == "no: rename the parser first")
        self.assertNotIn("now add the tests", jr.render_judge(rewound), "the abandoned branch leaked")

    def test_quotes_resolve_to_utf8_bytes_and_paraphrases_do_not(self):
        text = "naïve café — ok\n  then   more"
        self.assertEqual(jr.resolve(text, "café — ok"), [7, 19])
        self.assertEqual(jr.resolve(text, "ok then more"), [17, 33])
        self.assertIsNone(jr.resolve(text, "a naive cafe"))
        self.assertIsNone(jr.resolve(text, ""))
        self.assertIsNone(jr.cite(boundary(["x"]), {"msg": "u2", "quote": "x"})["bytes"])

    def test_the_intent_cap_speaks_at_limit_plus_one(self):
        row = boundary(["a" * 10, "b" * 20])
        limit = len("[u1] " + "a" * 10) + 1 + len("[u2] " + "b" * 20) + 1
        whole = jr.render_judge(row, cap=limit)
        self.assertNotIn("[…", whole)
        self.assertIn("[u1] " + "a" * 10, whole)
        cut = jr.render_judge(boundary(["a" * 11, "b" * 20]), cap=limit)
        self.assertIn(f"[… kept the newest 1 of 2 messages; intent_chars={limit} cut u1..u1;", cut)
        self.assertNotIn("[u1]", cut)
        self.assertIn("[u2] " + "b" * 20, cut)

    def test_the_call_head_cap_speaks_at_limit_plus_one(self):
        at = jr.render_judge(boundary(["x"], [{"name": "bash", "head": "c" * jr.DIGEST_HEAD}]))
        self.assertNotIn("[…", at)
        over = jr.render_judge(boundary(["x"], [{"name": "bash", "head": "c" * (jr.DIGEST_HEAD + 1)}]))
        self.assertIn(f"[… 1 of 1 call heads cut to digest_head={jr.DIGEST_HEAD} chars;", over)

    def test_parallel_calls_share_one_home(self):
        saved, jr._HOME[:] = list(jr._HOME), []
        self.addCleanup(lambda: jr._HOME.__setitem__(slice(None), saved))
        start, homes, errors = threading.Barrier(16), [], []

        def call():
            start.wait()
            try:
                homes.append(jr.home())
            except OSError as error:
                errors.append(error)

        threads = [threading.Thread(target=call) for _ in range(16)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual((errors, len(set(homes))), ([], 1))

    def test_an_input_past_argv_bytes_is_refused_and_named(self):
        args = types.SimpleNamespace(binary="/usr/bin/false", model="faux/faux-1", out=tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, args.out, True)
        at = jr.ask(args, "judge", "s", "x" * (jr.ARGV_BYTES - 1))
        self.assertTrue(at["error"].startswith("exit 1"), at)
        over = jr.ask(args, "judge", "s", "x" * jr.ARGV_BYTES)
        self.assertEqual(over, {"error": f"input over argv_bytes={jr.ARGV_BYTES}"})


class Metrics(unittest.TestCase):
    def test_balanced_accuracy_on_a_hand_table(self):
        pairs = [(True, True)] * 3 + [(True, False)] + [(False, False)] * 2 + [(False, True)] * 2
        self.assertEqual(jr.balanced(pairs), (0.625, 0.75, 0.5))
        self.assertIsNone(jr.balanced([(True, True)]), "specificity is undefined with no negatives")
        self.assertEqual(jr.balanced([(True, False), (False, False)])[0], 0.5)
        self.assertEqual(jr.balanced([(True, True), (False, True)])[0], 0.5)

    def test_the_bootstrap_resamples_sessions(self):
        # A session resample is s1+s1 (1.0), s2+s2 (0.0) or s1+s2 (0.5); resampling the 40 rows
        # instead would pile up near 0.5 and hide how much one session decides.
        by_session = {"s1": [(True, True)] * 10 + [(False, False)] * 10,
                      "s2": [(True, False)] * 10 + [(False, True)] * 10}
        self.assertEqual(jr.balanced(by_session["s1"] + by_session["s2"])[0], 0.5)
        self.assertEqual(jr.bootstrap(by_session), (0.0, 1.0))
        self.assertIsNone(jr.bootstrap({"s1": [(True, True)]}))

    def test_the_gate_line_at_its_thresholds(self):
        held = {"resolution": 0.95, "interval": (0.501, 0.9), "tooled": 0, "missing": 0}
        self.assertTrue(jr.gate_line(held).startswith("gate: PASS"))
        for change in ({"interval": (0.5, 0.9)}, {"resolution": 0.949}, {"resolution": None, "interval": None},
                       {"tooled": 1}, {"missing": 1}):
            self.assertTrue(jr.gate_line({**held, **change}).startswith("gate: FAIL"), change)

    def test_a_session_never_lands_on_both_sides(self):
        rows = jr.extract_corpora([("yi", jr.FIXTURES["yi"]), ("claude", CLAUDE)])
        sides = {}
        for row in rows:
            sides.setdefault(row["session"], set()).add(row["split"])
        self.assertTrue(all(len(s) == 1 for s in sides.values()), sides)
        fit = sum(jr.split_of("claude", f"{n}.jsonl") == "fit" for n in range(2000))
        self.assertTrue(1300 <= fit <= 1500, fit)


if __name__ == "__main__":
    unittest.main()
