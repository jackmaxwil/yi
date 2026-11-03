"""yi.convert: the document converter's pure parts, judged against markdown recorded from anydoc and the workbooks' own cells."""
import json
import unittest
from pathlib import Path

from yi import convert

VECTORS = json.loads((Path(__file__).parent / "vectors" / "convert.json").read_text(encoding="utf-8"))


def table_rows(markdown):
    rows, name = {}, None
    for line in markdown.splitlines():
        if line.startswith("## "):
            name = line[3:].strip()
        elif line.startswith("|"):
            cells = convert.cells_of(line)
            if not all(cell.strip("-: ") == "" for cell in cells):
                rows.setdefault(name, []).append(cells)
    return rows


class TidySheets(unittest.TestCase):
    def test_roster_keeps_the_real_columns_and_rows_of_each_sheet(self):
        vector = VECTORS["roster.xlsx"]
        names = list(vector["sheets"])
        tidy = convert.tidy_sheets(vector["markdown"], names, True)
        self.assertTrue(tidy.startswith("[sheets: Roster 3x3, Notes 1x2]"), tidy)
        found = table_rows(tidy)
        for sheet, cells in vector["sheets"].items():
            rows = [row for row in cells if any(row)]
            columns = [c for c in range(len(rows[0])) if any(row[c] for row in rows)]
            expected = [[" ".join(row[c].split()) for c in columns] for row in rows]
            got = [row for row in found[sheet]]
            self.assertEqual(len(got), len(expected), (sheet, got))
            for want, have in zip(expected, got):
                self.assertEqual(want[0], have[0], (sheet, want, have))
                self.assertEqual(len(want), len(have), (sheet, want, have))

    def test_a_long_sheet_is_listed_with_its_row_count_and_a_pandas_hint(self):
        vector = VECTORS["ledger.xlsx"]
        tidy = convert.tidy_sheets(vector["markdown"], ["Ledger"], True)
        rows = len(vector["sheets"]["Ledger"]) - 1
        self.assertTrue(tidy.startswith(f"[sheets: Ledger {rows}x2 — for analysis, pandas.read_excel"), tidy[:120])
        self.assertNotIn("pandas", convert.tidy_sheets(vector["markdown"], ["Ledger"], False))


class RunningLines(unittest.TestCase):
    def test_a_multi_page_pdf_loses_its_running_header_and_keeps_its_body(self):
        pages = VECTORS["running.pdf"]["pages"]
        chunks = {number: [page.strip()] for number, page in enumerate(pages, 1)}
        running = convert.running_lines(chunks)
        self.assertEqual(running, {"pages.txt #-#-# #:#"})
        for number, chunk in chunks.items():
            kept = convert.trim(chunk[0], running)
            self.assertNotIn("pages.txt", kept)
            self.assertEqual(kept.count("Line of body text number"), chunk[0].count("Line of body text number"))

    def test_body_lines_numbered_alike_are_not_mistaken_for_a_header(self):
        body = "\n".join(f"Line of body text number {n}" for n in range(1, 50))
        self.assertEqual(convert.running_lines({n: [body] for n in range(1, 6)}), set())


class Shape(unittest.TestCase):
    def test_digits_and_markup_collapse(self):
        self.assertEqual(convert.shape("## **Page 12** of 40"), "Page # of #")


class MainReply(unittest.TestCase):
    def test_detection_failure_is_a_json_error_line_not_a_crash(self):
        import io
        import sys
        import types
        from contextlib import redirect_stdout
        from unittest import mock

        class Unsupported(Exception):
            pass

        def boom(_data):
            raise ValueError("bad header")

        stub = types.SimpleNamespace(
            format_from_bytes=boom,
            UnsupportedError=Unsupported,
            EncryptedError=type("Enc", (Exception,), {}),
            ResourceLimitError=type("Lim", (Exception,), {}),
        )
        out = io.StringIO()
        with mock.patch.dict(sys.modules, {"anydoc": stub}), mock.patch.object(sys, "stdin", types.SimpleNamespace(buffer=io.BytesIO(b"x"))), redirect_stdout(out):
            convert.main(["f.pdf", "/dev/null", "null"])
        self.assertEqual(json.loads(out.getvalue().splitlines()[-1]), {"error": "not readable as this format: bad header"})

if __name__ == "__main__":
    unittest.main()
