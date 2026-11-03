"""The document converter the read tool runs: ``python -I -c <this file> <source> <staging> <pages>``.

The binary carries these bytes and hands them over with ``-c``, so a conversion never depends on
the ``yi`` package installed in the kernel venv; ``python -m yi.convert`` runs the same program.
The document arrives on stdin, the Markdown is written to ``<staging>``, and the last stdout line
is one JSON object: ``ok``, ``unsupported``, ``missing``, ``ocr`` with ``page_count``, or
``error``. ``anydoc`` and ``pdf_inspector`` are imported by the function that uses them, so
importing this module needs neither and the stdlib test lane runs everything else.
``ocr="reject"`` is the only mode ever passed: anydoc's hosted OCR uploads the document, and no
argument, setting or message leads there.
"""
from __future__ import annotations

import io
import json
import re
import sys
import zipfile


class Reply(Exception):
    def __init__(self, **fields):
        super().__init__(fields)
        self.fields = fields


def cells_of(line):
    return [cell.strip() for cell in re.split(r"(?<!\\)\|", line)[1:-1]]


def sheet_names(data):
    try:
        workbook = zipfile.ZipFile(io.BytesIO(data)).read("xl/workbook.xml").decode("utf-8")
    except Exception:
        return []
    return re.findall(r'<sheet\b[^>]*\bname="([^"]*)"', workbook)


def zipped_workbook(data, source):
    return data[:2] == b"PK" and not source.lower().endswith(".xlsb")


def tidy_sheets(markdown, names, pandas_reads):
    blocks, sheets, current = [], [], None
    for line in markdown.splitlines():
        if line.startswith("|"):
            if current is None:
                current = []
                blocks.append(current)
            current.append(line)
        else:
            current = None
            blocks.append(line)
    text, name = [], None
    for block in blocks:
        if isinstance(block, str):
            if block.startswith("## "):
                name = block[3:].strip()
            text.append(block)
            continue
        rows = [cells_of(re.sub(r"[ \t]{2,}", " ", line)) for line in block]
        width = max(len(row) for row in rows)
        rows = [row + [""] * (width - len(row)) for row in rows]
        keep = [c for c in range(width) if any(row[c] and not re.fullmatch(r":?-+:?", row[c]) for row in rows)]
        rows = [[row[c] for c in keep] for row in rows]
        body = [row for index, row in enumerate(rows) if index <= 1 or any(row)]
        if not keep:
            continue
        if len(body) > 3 and not any(body[0]):
            body = [body[2], body[1]] + body[3:]
        separated = len(body) > 1 and all(re.fullmatch(r":?-+:?", cell) for cell in body[1] if cell)
        fallback = names[len(sheets)] if len(sheets) < len(names) else "sheet"
        sheets.append((name or fallback, len(body) - (2 if separated else 0), len(keep)))
        text.extend("| " + " | ".join(row) + " |" for row in body)
    result = "\n".join(text) + "\n"
    big = any(rows > 500 for _, rows, _ in sheets)
    if len(sheets) > 1 or big:
        listing = ", ".join(f"{name} {rows}x{cols}" for name, rows, cols in sheets)
        hint = " — for analysis, pandas.read_excel in ipython reads it whole" if big and pandas_reads else ""
        result = f"[sheets: {listing}{hint}]\n\n{result}"
    return result


def pptx_slides(data):
    import anydoc

    archive = zipfile.ZipFile(io.BytesIO(data))
    presentation = archive.read("ppt/presentation.xml").decode("utf-8")
    listing = re.search(r"<p:sldIdLst>(.*?)</p:sldIdLst>", presentation, re.S)
    slides = re.findall(r"<p:sldId\b[^>]*?/>", listing.group(1)) if listing else []
    if len(slides) < 2 or len(slides) * len(data) > 300_000_000:
        return None
    members = [(info, archive.read(info.filename)) for info in archive.infolist()]
    parts = []
    for number, slide in enumerate(slides, 1):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as single:
            for info, body in members:
                if info.filename == "ppt/presentation.xml":
                    body = (presentation[: listing.start(1)] + slide + presentation[listing.end(1):]).encode("utf-8")
                single.writestr(info, body)
        text = anydoc.to_markdown_bytes(buffer.getvalue(), "pptx", ocr="reject").strip()
        parts.append(f"[slide {number}]\n{text}" if text else f"[slide {number}: no text]")
    return "\n\n".join(parts) + "\n"


def page_regions(data, indices, columned):
    """A table page keeps its full width, so a row reads left to right; a columned page is read
    left column then right, split at the gutter the fewest text runs cross."""
    import pdf_inspector

    spans = {}
    wanted = [index + 1 for index in indices if index + 1 in columned]
    for item in pdf_inspector.extract_text_with_positions_bytes(data, pages=wanted) if wanted else []:
        if item.text.strip():
            spans.setdefault(item.page, []).append((item.x, item.x + item.width))
    regions = []
    for index in indices:
        boxes = [[0, 0, 100000, 100000]]
        here = spans.get(index + 1)
        if here:
            lo, hi = min(a for a, _ in here), max(b for _, b in here)
            crossing = lambda g: sum(1 for a, b in here if a < g < b)
            gutter = min((lo + (hi - lo) * k / 60 for k in range(20, 41)), key=crossing)
            if crossing(gutter) * 20 < len(here):
                boxes = [[0, 0, gutter, 100000], [gutter, 0, 100000, 100000]]
        regions.append((index, boxes))
    found = pdf_inspector.extract_text_in_regions_bytes(data, regions)
    return {page.page: [region.text.strip() for region in page.regions] for page in found}


def shape(line):
    return re.sub(r"\d+", "#", re.sub(r"^[#>*\s]+|[*_`]", "", line)).strip()


def running_lines(chunks):
    """A line at the edge of at least 40% of pages (and three), and never inside one, is a
    running header or footer; numbered body lines share a shape but also fill the middle."""
    edge_seen, inside = {}, set()
    for page_chunks in chunks.values():
        edges = set()
        for chunk in page_chunks:
            lines = [line for line in chunk.splitlines() if line.strip()]
            edges.update(shape(line) for line in lines[:2] + lines[-2:])
            inside.update(shape(line) for line in lines[2:-2])
        for edge in edges:
            edge_seen[edge] = edge_seen.get(edge, 0) + 1
    floor = max(3, len(chunks) * 0.4)
    return {edge for edge, count in edge_seen.items() if count >= floor and edge and edge not in inside}


def trim(chunk, running):
    lines = chunk.splitlines()
    filled = [index for index, line in enumerate(lines) if line.strip()]
    edges = set(filled[:2] + filled[-2:])
    kept = [line for index, line in enumerate(lines) if not (index in edges and shape(line) in running)]
    return "\n".join(kept).strip("\n")


def with_headings(markdown, text):
    levels = {}
    for line in markdown.splitlines():
        heading = re.match(r"(#{1,6}) +(.*\S)", line)
        if heading:
            levels.setdefault(re.sub(r"[*_`]", "", heading[2]).strip(), heading[1])
    return "\n".join(f"{levels[line.strip()]} {line.strip()}" if line.strip() in levels else line for line in text.splitlines())


def pdf_markdown(data, pages):
    import pdf_inspector

    count = pdf_inspector.detect_pdf_bytes(data).page_count
    wanted = [page for page in pages if 1 <= page <= count] if pages else list(range(1, count + 1))
    if pages and not wanted:
        raise Reply(error=f"the PDF has {count} page(s); none of the pages asked for exist")
    result = pdf_inspector.extract_pages_markdown_bytes(data, pages=[page - 1 for page in wanted])
    tables, columns = set(result.pages_with_tables or []), set(result.pages_with_columns or [])
    flat = [page.page for page in result.pages if not page.needs_ocr and page.markdown.strip() and (page.page + 1 in tables or page.page + 1 in columns)]

    def widest(markdown):
        return max((len(cells_of(line)) for line in markdown.splitlines() if line.startswith("|")), default=0)

    split = {page.page + 1 for page in result.pages if page.page + 1 in columns and (page.page + 1 not in tables or widest(page.markdown) <= 2)}
    plain = page_regions(data, flat, split) if flat else {}
    chunks, notes, blank = {}, {}, 0
    for page in result.pages:
        number = page.page + 1
        if page.needs_ocr or not page.markdown.strip():
            blank += 1
            notes[number] = "no text layer"
            chunks[number] = []
        elif page.page in plain:
            chunks[number] = [with_headings(page.markdown, text) for text in plain[page.page]]
            what = "two columns" if number in split else "a table"
            notes[number] = f"{what} flattened to plain text in reading order; cell and column boundaries are lost"
        else:
            chunks[number] = [page.markdown.strip()]
    if blank == len(wanted):
        raise Reply(ocr=blank, page_count=count)
    running = running_lines(chunks) if len(chunks) >= 3 else set()
    parts = []
    if running:
        named = [line for line in sorted(running) if len(line) >= 3][:3]
        parts.append("[running headers and footers left out" + (": " + "; ".join(named) if named else "") + "]")
    single = count == 1 and not pages
    for number in sorted(chunks):
        text = "\n".join(filter(None, (trim(chunk, running) for chunk in chunks[number])))
        note = notes.get(number)
        head = f"[{note}]" if single and note else "" if single else f"[page {number}: {note}]" if note else f"[page {number}]"
        parts.append("\n".join(filter(None, [head, text])))
    return "\n\n".join(parts) + "\n"


def convert(source, pages, data):
    try:
        import anydoc
    except ImportError as error:
        raise Reply(missing=str(error))
    kind = "this format"
    try:
        kind = anydoc.format_from_bytes(data) or anydoc.format_from_path(source)
        if kind in (None, "csv"):
            raise Reply(unsupported=True)
        if kind == "pdf":
            markdown = pdf_markdown(data, pages)
        elif kind == "pptx":
            markdown = pptx_slides(data) or anydoc.to_markdown_bytes(data, kind, ocr="reject")
        else:
            markdown = anydoc.to_markdown_bytes(data, kind, ocr="reject")
            if kind in ("xlsx", "ods"):
                markdown = tidy_sheets(markdown, sheet_names(data), zipped_workbook(data, source))
    except Reply:
        raise
    except anydoc.UnsupportedError:
        raise Reply(unsupported=True)
    except anydoc.EncryptedError:
        raise Reply(error="encrypted; it cannot be read without its password")
    except anydoc.ResourceLimitError as error:
        spreadsheet = kind in ("xlsx", "ods") and zipped_workbook(data, source)
        hint = "; in ipython, pandas.read_excel reads a spreadsheet of this size" if spreadsheet else ""
        raise Reply(error=f"too large for the converter ({error}){hint}")
    except Exception as error:
        raise Reply(error=f"not readable as {kind}: {error}")
    return kind, markdown


def main(argv):
    source, out, pages = argv[0], argv[1], json.loads(argv[2])
    try:
        kind, markdown = convert(source, pages, sys.stdin.buffer.read())
        with open(out, "w", encoding="utf-8") as handle:
            handle.write(markdown)
        raise Reply(ok=True, kind=kind)
    except Reply as reply:
        print(json.dumps(reply.fields))


if __name__ == "__main__":
    main(sys.argv[1:])
