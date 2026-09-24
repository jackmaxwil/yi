"""F0b: the Python twin of `plan_journal::canonical_hashes_match_across_rust_and_python`.

The canonical rule is `crates/runtime/tests/fixtures/plans/journal/canonical.md`; both sides
read the same vectors and the same journal fixture, so a difference is a bug in one of them,
never a reason to regenerate. stdlib only. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import hashlib
import json
import pathlib
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[2]
VECTORS = HERE / "vectors" / "canonical.json"
JOURNAL = ROOT / "crates" / "runtime" / "tests" / "fixtures" / "plans" / "journal"


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(
        "utf-8"
    )


def digest(prev: str | None, record: dict) -> str:
    without = {key: value for key, value in record.items() if key != "digest"}
    hasher = hashlib.sha256()
    if prev is not None:
        hasher.update(prev.encode("utf-8"))
    hasher.update(canonical(without))
    return "sha256:" + hasher.hexdigest()


class CanonicalVectors(unittest.TestCase):
    def test_vectors_canonicalize_and_hash_as_written(self) -> None:
        document = json.loads(VECTORS.read_text(encoding="utf-8"))
        cases = document["vectors"]
        self.assertGreaterEqual(len(cases), 10)
        for case in cases:
            with self.subTest(case["name"]):
                encoded = canonical(case["input"])
                self.assertEqual(encoded.decode("utf-8"), case["canonical"])
                self.assertEqual(hashlib.sha256(encoded).hexdigest(), case["sha256"])

    def test_the_journal_fixture_chains(self) -> None:
        prev = None
        seen = 0
        for line in (JOURNAL / "records.jsonl").read_text(encoding="utf-8").splitlines():
            record = json.loads(line)
            self.assertEqual(
                record["argsHash"], "sha256:" + hashlib.sha256(canonical(record["args"])).hexdigest()
            )
            self.assertEqual(record["digest"], digest(prev, record))
            self.assertEqual(canonical(record).decode("utf-8"), line)
            prev = record["digest"]
            seen += 1
        self.assertEqual(seen, 8)

    def test_a_damaged_middle_record_breaks_the_chain_at_two(self) -> None:
        prev = None
        broken_at = None
        for line in (JOURNAL / "damaged-middle.jsonl").read_text(encoding="utf-8").splitlines():
            record = json.loads(line)
            if record["digest"] != digest(prev, record):
                broken_at = record["seq"]
                break
            prev = record["digest"]
        self.assertEqual(broken_at, 2)


if __name__ == "__main__":
    unittest.main()
