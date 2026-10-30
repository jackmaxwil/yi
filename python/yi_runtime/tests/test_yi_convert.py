import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve()
SRC = HERE.parents[1] / "src"
FIXTURES = HERE.parents[3] / "crates" / "runtime" / "tests" / "fixtures" / "documents"


def wheels() -> bool:
    try:
        import anydoc  # noqa: F401
        import pdf_inspector  # noqa: F401
    except ImportError:
        return False
    return True


@unittest.skipUnless(FIXTURES.is_dir(), "the document fixtures are not beside this package")
@unittest.skipUnless(wheels(), "the venv carries no anydoc, so only the venv lane converts")
class ConvertTests(unittest.TestCase):
    def convert(self, name: str, pages: list[int]) -> tuple[dict, str]:
        """Run the module the host will call: argv source, out, pages; the bytes on stdin."""
        source = FIXTURES / name
        out = pathlib.Path(tempfile.mkdtemp()) / "out.md"
        env = dict(os.environ, PYTHONPATH=str(SRC))
        with source.open("rb") as handle:
            finished = subprocess.run(
                [sys.executable, "-m", "yi.convert", str(source), str(out), json.dumps(pages)],
                stdin=handle,
                capture_output=True,
                env=env,
                check=True,
            )
        reply = json.loads(finished.stdout.decode().strip().splitlines()[-1])
        return reply, out.read_text() if out.exists() else ""

    def test_a_word_document_keeps_its_heading_and_its_non_ascii_cells(self) -> None:
        reply, markdown = self.convert("brief.docx", [])
        self.assertEqual((reply.get("ok"), reply.get("kind")), (True, "docx"), reply)
        self.assertIn("# Quarterly brief", markdown)
        self.assertIn("| Espresso machine | Zoë | ordered |", markdown)

    def test_a_spreadsheet_row_is_past_the_page_cut_and_named_by_pandas(self) -> None:
        reply, markdown = self.convert("ledger.xlsx", [])
        self.assertEqual((reply.get("ok"), reply.get("kind")), (True, "xlsx"), reply)
        self.assertIn("| 1 | 10 |", markdown)
        self.assertIn("pandas.read_excel", markdown)

    def test_a_page_without_a_text_layer_is_counted_for_ocr(self) -> None:
        reply, markdown = self.convert("mixed.pdf", [2])
        self.assertEqual(reply.get("ocr"), 1, reply)
        self.assertEqual(markdown, "", "a page with no text layer wrote a markdown body")

    def test_a_page_selection_keeps_the_pages_asked_for_and_names_the_blank_one(self) -> None:
        reply, markdown = self.convert("mixed.pdf", [])
        self.assertEqual(reply.get("kind"), "pdf", reply)
        self.assertIn("Zoë signs the lease", markdown)
        self.assertIn("[page 2: no text layer]", markdown)

    def test_a_page_past_the_end_is_a_refusal_naming_the_count(self) -> None:
        reply, _ = self.convert("mixed.pdf", [9])
        self.assertIn("has 2 page(s)", reply.get("error", ""), reply)

    def test_plain_text_is_not_a_document(self) -> None:
        source = pathlib.Path(tempfile.mkdtemp()) / "notes.txt"
        source.write_text("plain\n")
        out = source.with_suffix(".md")
        with source.open("rb") as handle:
            finished = subprocess.run(
                [sys.executable, "-m", "yi.convert", str(source), str(out), "[]"],
                stdin=handle,
                capture_output=True,
                env=dict(os.environ, PYTHONPATH=str(SRC)),
                check=True,
            )
        reply = json.loads(finished.stdout.decode().strip().splitlines()[-1])
        self.assertEqual(reply, {"unsupported": True})
        self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
