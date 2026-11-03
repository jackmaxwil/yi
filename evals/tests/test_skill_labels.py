"""The classifier's labelling tool on synthetic transcripts: no model, no network, no paid run."""
import csv, json, pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import skill_labels  # noqa: E402

PLANT = "sk-fake-3QpZr7Lm2Xv9Tb4Nc8Kd1Wq6"


def claude_line(**entry):
    return json.dumps(entry)


def transcript():
    return "\n".join([
        claude_line(type="user", message={"content": f"land this branch; key {PLANT} <system-reminder>noise</system-reminder>"}),
        claude_line(type="assistant", message={"content": [{"type": "tool_use", "name": "Skill", "input": {"skill": "yi-forge"}}]}),
        claude_line(type="user", isMeta=True, message={"content": "meta text"}),
        claude_line(type="user", isSidechain=True, message={"content": "a parent's prompt to a child"}),
        claude_line(type="user", message={"content": "<command-name>/clear</command-name>"}),
        claude_line(type="user", message={"content": [{"type": "tool_result", "content": "output"}]}),
        claude_line(type="user", message={"content": "Land this branch;  key " + PLANT}),
        claude_line(type="user", message={"content": "review the work"}),
    ])


class SkillLabels(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp())
        (self.dir / "claude" / "proj").mkdir(parents=True)
        (self.dir / "claude" / "proj" / "s.jsonl").write_text(transcript())
        yi = self.dir / "yi" / "lane"
        yi.mkdir(parents=True)
        (yi / "s.jsonl").write_text("\n".join([
            json.dumps({"type": "message", "message": {"role": "user", "attribution": "user", "content": "run cargo nextest"}}),
            json.dumps({"type": "message", "message": {"role": "user", "content": "a host-written prompt"}}),
        ]))
        self.corpus = self.dir / "corpus.jsonl"

    def test_a_message_id_matches_the_runtime(self):
        """yi-runtime's classifier_e2e::a_message_id_matches_the_labelling_tool pins the same value."""
        self.assertEqual(skill_labels.message_id("  Land THIS branch\n\tonce the ÉTÉ gate is green "), "2990f761ef69")

    def test_a_message_id_splits_on_the_runtimes_whitespace_only(self):
        """Rust's is_ascii_whitespace leaves out the vertical tab; a lone surrogate must not crash."""
        self.assertEqual(skill_labels.message_id("a\vb"), "98992d7f2eec")
        self.assertEqual(skill_labels.message_id("a\x0cb"), skill_labels.message_id("a b"))
        self.assertEqual(len(skill_labels.message_id("a \ud800")), 12)

    def test_a_killed_run_resumes_past_its_torn_line_and_odd_answers_are_invalid(self):
        skill_labels.corpus(self.dir / "claude", self.dir / "yi", self.corpus)
        out = self.dir / "labels.jsonl"
        out.write_text('{"id": "torn", "sk')
        answers = iter(['{"skill": ["land"]}', '["land"]', '{"skill": 3}'])
        on_disk = []

        def teacher(text):
            on_disk.append(len(out.read_text().splitlines()))
            return next(answers), 0.0

        count, _ = skill_labels.label(self.corpus, out, "m", 9.0, None, teacher=teacher)
        self.assertEqual(on_disk, [1, 2, 3], "each row is on disk before the next call is paid for")
        self.assertEqual(count, 3)
        rows = [skill_labels.parsed(line) for line in out.read_text().splitlines()]
        self.assertEqual([row and row["skill"] for row in rows], [None, "invalid", "invalid", "invalid"])
        self.assertEqual(skill_labels.freeze(self.corpus, out, self.dir / "frozen.csv", 1, 1, 1), 0)

    def test_a_yi_message_keeps_its_edges_and_a_surrogate_reaches_the_sheet(self):
        lane = self.dir / "yi" / "odd"
        lane.mkdir()
        rows = [{"type": "message", "message": {"role": "user", "attribution": "user", "content": text}}
                for text in ["fix it\u3000", "hi \ud800", "fix\u2028it"]]
        (lane / "s.jsonl").write_text("\n".join(json.dumps(row, ensure_ascii=index == 1) for index, row in enumerate(rows)))
        skill_labels.corpus(self.dir / "empty", lane, self.corpus)
        ids = [json.loads(line)["id"] for line in self.corpus.read_text().splitlines()]
        self.assertEqual(ids, [skill_labels.message_id(text) for text in ["fix it\u3000", "hi \ud800", "fix\u2028it"]])
        out = self.dir / "labels.jsonl"
        out.write_bytes(b'\xff\n{"id": "%s"}\n' % ids[0].encode())
        skill_labels.label(self.corpus, out, "m", 9.0, None, teacher=lambda text: ('{"skill": "none"}', 0.0))
        self.assertEqual(skill_labels.freeze(self.corpus, out, self.dir / "frozen.csv", 1, 3, 1), 3)

    def test_the_candidates_are_the_shipped_skills_with_a_trigger(self):
        found = dict(skill_labels.skills())
        self.assertIn("verify", found)
        self.assertEqual(len(found), 10)
        self.assertTrue(all(text and not text.startswith(">") for text in found.values()), found)

    def test_only_typed_messages_enter_scrubbed_and_once(self):
        count = skill_labels.corpus(self.dir / "claude", self.dir / "yi", self.corpus)
        rows = [json.loads(line) for line in self.corpus.read_text().splitlines()]
        self.assertEqual(count, 3, rows)
        texts = [row["text"] for row in rows]
        self.assertTrue(texts[0].startswith("land this branch"), texts)
        self.assertNotIn(PLANT, self.corpus.read_text())
        self.assertNotIn("noise", texts[0])
        self.assertEqual(rows[0]["loaded"], "yi-forge")
        self.assertEqual([row["source"] for row in rows], ["claude", "claude", "yi"])
        self.assertIn("run cargo nextest", texts)

    def test_labels_resume_stop_at_the_budget_and_refuse_a_name_off_the_list(self):
        skill_labels.corpus(self.dir / "claude", self.dir / "yi", self.corpus)
        out = self.dir / "labels.jsonl"
        answers = iter(['{"skill": "land"}', '{"skill": "yi-forge"}', '{"skill": "none"}'])
        teacher = lambda text: (next(answers), 0.5)  # noqa: E731
        count, spent = skill_labels.label(self.corpus, out, "m", max_usd=1.0, limit=None, teacher=teacher)
        self.assertEqual((count, spent), (2, 1.0), "the budget stops the run")
        count, _ = skill_labels.label(self.corpus, out, "m", max_usd=1.0, limit=None, teacher=teacher)
        self.assertEqual(count, 1, "a resumed run labels only what is left")
        labels = [json.loads(line)["skill"] for line in out.read_text().splitlines()]
        self.assertEqual(labels, ["land", "invalid", "none"])

    def test_the_frozen_sample_is_stratified_and_leaves_the_owner_column_empty(self):
        skill_labels.corpus(self.dir / "claude", self.dir / "yi", self.corpus)
        out = self.dir / "labels.jsonl"
        answers = iter(['{"skill": "land"}', '{"skill": "review"}', '{"skill": "none"}'])
        skill_labels.label(self.corpus, out, "m", 9.0, None, teacher=lambda text: (next(answers), 0.0))
        frozen = self.dir / "frozen.csv"
        count = skill_labels.freeze(self.corpus, out, frozen, per_skill=1, none=0, seed=1)
        with frozen.open() as sheet:
            rows = list(csv.DictReader(sheet))
        self.assertEqual(count, 2)
        self.assertEqual(sorted(row["teacher"] for row in rows), ["land", "review"])
        self.assertTrue(all(row["owner"] == "" for row in rows))


if __name__ == "__main__":
    unittest.main()
