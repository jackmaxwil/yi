use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::sandbox::Sandbox;
use crate::tool::CancelFlag;

/// A document is attacker-shaped parser input; past this size it is refused, never parsed. A
/// page selection parses only those pages, so it is allowed further.
pub const INPUT_CEILING: u64 = 256 * 1024 * 1024;
const PAGED_CEILING: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn document_ceiling() -> u64 {
    INPUT_CEILING
}
const STEM_CHARS: usize = 64;
const HEAD: usize = 8 * 1024;
const CACHE_CAP: u64 = 256 * 1024 * 1024;
const CACHE_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const STAGING_AGE: Duration = Duration::from_secs(3600);
/// `ocr="reject"` is the only mode ever passed: anydoc's hosted OCR uploads the document, and no
/// argument, setting or message leads there. Content markers name the format before the extension.
const CONVERT: &str = r###"import io, json, re, sys, zipfile
source, out, pages = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
def say(**fields):
    print(json.dumps(fields))
    sys.exit()
try:
    import anydoc, pdf_inspector
except ImportError as error:
    say(missing=str(error))
with open(source, "rb") as handle:
    data = handle.read()
kind = anydoc.format_from_bytes(data) or anydoc.format_from_path(source)
if kind in (None, "csv"):
    say(unsupported=True)
def cells_of(line):
    return [cell.strip() for cell in re.split(r"(?<!\\)\|", line)[1:-1]]
def sheet_names():
    try:
        workbook = zipfile.ZipFile(io.BytesIO(data)).read("xl/workbook.xml").decode("utf-8")
    except Exception:
        return []
    return re.findall(r'<sheet\b[^>]*\bname="([^"]*)"', workbook)
def tidy_sheets(markdown):
    blocks, sheets, current, names = [], [], None, sheet_names()
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
        hint = " — for analysis, pandas.read_excel in ipython reads it whole" if big and data[:2] == b"PK" and not source.lower().endswith(".xlsb") else ""
        result = f"[sheets: {listing}{hint}]\n\n{result}"
    return result
def pptx_slides():
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
def page_regions(indices, columned):
    """A table page keeps its full width, so a row reads left to right; a columned page is read
    left column then right, split at the gutter the fewest text runs cross."""
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
def pdf_markdown():
    count = pdf_inspector.detect_pdf_bytes(data).page_count
    wanted = [page for page in pages if 1 <= page <= count] if pages else list(range(1, count + 1))
    if pages and not wanted:
        say(error=f"the PDF has {count} page(s); none of the pages asked for exist")
    result = pdf_inspector.extract_pages_markdown_bytes(data, pages=[page - 1 for page in wanted])
    tables, columns = set(result.pages_with_tables or []), set(result.pages_with_columns or [])
    flat = [page.page for page in result.pages if not page.needs_ocr and page.markdown.strip() and (page.page + 1 in tables or page.page + 1 in columns)]
    def widest(markdown):
        return max((len(cells_of(line)) for line in markdown.splitlines() if line.startswith("|")), default=0)
    split = {page.page + 1 for page in result.pages if page.page + 1 in columns and (page.page + 1 not in tables or widest(page.markdown) <= 2)}
    plain = page_regions(flat, split) if flat else {}
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
        say(ocr=blank, page_count=count)
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
try:
    if kind == "pdf":
        markdown = pdf_markdown()
    elif kind == "pptx":
        markdown = pptx_slides() or anydoc.to_markdown_bytes(data, kind, ocr="reject")
    else:
        markdown = anydoc.to_markdown_bytes(data, kind, ocr="reject")
        if kind in ("xlsx", "ods"):
            markdown = tidy_sheets(markdown)
except anydoc.UnsupportedError:
    say(unsupported=True)
except anydoc.EncryptedError:
    say(error="encrypted; it cannot be read without its password")
except anydoc.ResourceLimitError as error:
    spreadsheet = kind in ("xlsx", "ods") and data[:2] == b"PK" and not source.lower().endswith(".xlsb")
    hint = "; in ipython, pandas.read_excel reads a spreadsheet of this size" if spreadsheet else ""
    say(error=f"too large for the converter ({error}){hint}")
except Exception as error:
    say(error=f"not readable as {kind}: {error}")
with open(out, "w", encoding="utf-8") as handle:
    handle.write(markdown)
say(ok=True, kind=kind)
"###;

/// The venv python and the formats its wheel reported when it was built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Converter {
    pub python: PathBuf,
    pub formats: Vec<String>,
}

#[derive(Clone)]
pub struct Documents {
    pub home: PathBuf,
    /// Asked on every read and every description, so a venv built mid-session shows up.
    pub converter: Arc<dyn Fn() -> Converter + Send + Sync>,
    pub timeout: Duration,
}

impl Documents {
    pub fn fixed(home: PathBuf, converter: Converter) -> Self {
        Self {
            home,
            converter: Arc::new(move || converter.clone()),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

pub(crate) struct Source<'a> {
    pub path: &'a Path,
    pub bytes: &'a [u8],
    pub pages: Option<&'a str>,
}

pub(crate) struct Copy {
    pub path: PathBuf,
    pub kind: String,
    pub hit: bool,
    pub millis: u128,
}

pub(crate) enum Converted {
    Markdown(Copy),
    NotADocument,
    Unavailable(String),
    Refused(String),
}

pub(crate) fn describe(base: &str, formats: &[String]) -> String {
    if formats.is_empty() {
        return base.to_owned();
    }
    format!(
        "{base} Files in these formats are converted to Markdown: {}. The Markdown is read-only (edit and write refuse the original), files past 256 MiB are refused, PDF pages without a text layer are left out, a PDF page with a table or two columns arrives as plain text in reading order under a note saying so, and pages=\"3-5\" reads only those PDF pages.",
        formats.join(", ")
    )
}

pub(crate) fn has_nul(bytes: &[u8]) -> bool {
    bytes.iter().take(HEAD).any(|byte| *byte == 0)
}

fn after_bom(bytes: &[u8]) -> &[u8] {
    let rest = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let start = rest
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(rest.len());
    rest.get(start..).unwrap_or_default()
}

/// The four containers every convertible format lives in; nothing else is worth a spawn.
/// ponytail: a new container in anydoc means a fifth marker here, and the dogfood run finds it.
pub(crate) fn could_be_document(bytes: &[u8]) -> bool {
    let head = after_bom(bytes);
    head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"\xd0\xcf\x11\xe0")
        || head.starts_with(b"{\\rtf")
        || bytes
            .get(..1024.min(bytes.len()))
            .is_some_and(|window| window.windows(5).any(|w| w == b"%PDF-"))
}

fn is_rtf(bytes: &[u8]) -> bool {
    after_bom(bytes).starts_with(b"{\\rtf")
}

pub(crate) fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("PNG")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("JPEG")
    } else if bytes.starts_with(b"GIF8") {
        Some("GIF")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("WebP")
    } else {
        None
    }
}

pub(crate) fn python_str(path: &Path) -> String {
    serde_json::Value::from(path.to_string_lossy()).to_string()
}

/// UTF-16 by its byte-order mark, else Latin-1: every byte is a character, so nothing is lost.
pub(crate) fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    let pairs = |bytes: &[u8], big: bool| -> String {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| {
                let pair = [pair[0], pair[1]];
                if big {
                    u16::from_be_bytes(pair)
                } else {
                    u16::from_le_bytes(pair)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        (pairs(rest, false), "UTF-16")
    } else if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        (pairs(rest, true), "UTF-16")
    } else {
        (
            bytes.iter().map(|byte| char::from(*byte)).collect(),
            "Latin-1",
        )
    }
}

fn converted_dir(home: &Path) -> PathBuf {
    home.join(".yi").join("converted")
}

pub(crate) fn is_copy(home: &Path, path: &Path) -> bool {
    path.starts_with(converted_dir(home))
}

/// Refused on what the bytes are, not on whether a copy exists: a document written as text is
/// destroyed either way. RTF is text and stays writable.
pub(crate) fn write_refusal(home: Option<&Path>, path: &Path) -> Option<String> {
    if home.is_some_and(|home| is_copy(home, path)) {
        return Some(format!(
            "{} is the read-only Markdown copy of a document; nothing written here reaches the original. Change the original in the application that made it.",
            path.display()
        ));
    }
    let mut head = vec![0_u8; HEAD];
    let read = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
    head.truncate(read);
    let kind = if is_rtf(&head) {
        return None;
    } else if could_be_document(&head) {
        "a binary document"
    } else if has_nul(&head) {
        "a binary file"
    } else {
        return None;
    };
    Some(format!(
        "{} is {kind}; read shows it as read-only Markdown and nothing writes text back into it. Change the original in the application that made it, or write a new file beside it.",
        path.display()
    ))
}

type HashMemo = std::collections::HashMap<(PathBuf, u64, u128), String>;

/// Hashing a 48 MB source on every read cost 200 ms; a file whose mtime was at least two seconds
/// old when hashed cannot have been rewritten within the same timestamp tick, so its hash is kept.
fn source_hash(canonical: &Path, bytes: &[u8]) -> String {
    static KNOWN: std::sync::Mutex<Option<HashMemo>> = std::sync::Mutex::new(None);
    let stamp = std::fs::metadata(canonical)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok());
    let key = stamp.map(|stamp| {
        (
            canonical.to_path_buf(),
            bytes.len() as u64,
            stamp.as_nanos(),
        )
    });
    if let Some(key) = &key
        && let Some(hash) = KNOWN
            .lock()
            .ok()
            .and_then(|known| known.as_ref().and_then(|known| known.get(key).cloned()))
    {
        return hash;
    }
    let low = xxhash_rust::xxh32::xxh32(bytes, 0);
    let high = xxhash_rust::xxh32::xxh32(bytes, 1);
    let hash = format!("{high:08x}{low:08x}");
    if let (Some(key), Some(stamp)) = (key, stamp)
        && settled(stamp)
        && let Ok(mut known) = KNOWN.lock()
    {
        known
            .get_or_insert_with(Default::default)
            .insert(key, hash.clone());
    }
    hash
}

#[expect(
    clippy::disallowed_methods,
    reason = "whether a timestamp tick could still be open is a question about the clock"
)]
fn settled(modified: Duration) -> bool {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .is_ok_and(|now| now.saturating_sub(modified) >= Duration::from_secs(2))
}

/// Keyed by the bytes, so any save of the original misses and a same-second edit cannot be
/// served stale. The Markdown's own hash goes in the name, so a tampered copy is converted again.
fn cache_key(source: &Source<'_>) -> Option<String> {
    let canonical = source
        .path
        .canonicalize()
        .unwrap_or_else(|_| source.path.to_path_buf());
    let bytes_hash = source_hash(&canonical, source.bytes);
    let key = format!(
        "{}\0{}\0{bytes_hash}\0{}",
        canonical.display(),
        source.bytes.len(),
        source.pages.unwrap_or_default()
    );
    let stem: String = source
        .path
        .file_stem()?
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .take(STEM_CHARS)
        .collect();
    let a = xxhash_rust::xxh32::xxh32(key.as_bytes(), 0);
    let b = xxhash_rust::xxh32::xxh32(key.as_bytes(), 1);
    Some(format!("{stem}-{b:08x}{a:08x}"))
}

fn markdown_hash(text: &[u8]) -> String {
    format!("{:08x}", xxhash_rust::xxh32::xxh32(text, 7))
}

fn cached(dir: &Path, prefix: &str) -> Option<(PathBuf, String)> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(|rest| rest.strip_suffix(".md"))
        else {
            continue;
        };
        let Some((hash, kind)) = rest.split_once('.') else {
            continue;
        };
        let path = entry.path();
        match std::fs::read(&path) {
            Ok(text) if markdown_hash(&text) == hash => return Some((path, kind.to_owned())),
            _ => {
                let _tampered_or_unreadable = std::fs::remove_file(&path);
            }
        }
    }
    None
}

#[expect(
    clippy::disallowed_methods,
    reason = "eviction ages copies against the clock; nothing else here reads time"
)]
fn sweep(dir: &Path) {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut copies: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tmp") {
            if age > STAGING_AGE {
                let _left_by_a_killed_converter = std::fs::remove_file(&path);
            }
        } else if age > CACHE_AGE {
            let _expired = std::fs::remove_file(&path);
        } else {
            copies.push((modified, meta.len(), path));
        }
    }
    let mut total: u64 = copies.iter().map(|(_, len, _)| *len).sum();
    copies.sort();
    for (_, len, path) in copies {
        if total <= CACHE_CAP {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

fn parse_pages(spec: &str) -> Result<Vec<u64>, String> {
    let mut pages = Vec::new();
    for part in spec
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (lo, hi) = match part.split_once('-') {
            Some((lo, hi)) => (lo.trim(), hi.trim()),
            None => (part, part),
        };
        match (lo.parse::<u64>(), hi.parse::<u64>()) {
            (Ok(lo), Ok(hi)) if lo >= 1 && hi >= lo && hi - lo < 10_000 => pages.extend(lo..=hi),
            _ => {
                return Err(format!(
                    "pages={spec:?} is not a 1-based list like \"3-5,9\""
                ));
            }
        }
    }
    if pages.is_empty() {
        return Err(format!("pages={spec:?} names no page"));
    }
    Ok(pages)
}

/// The copy a previous conversion left, without converting: what a search or a listing may use.
pub(crate) fn peek(documents: &Documents, source: &Source<'_>) -> Option<Copy> {
    let prefix = cache_key(source)?;
    let (path, kind) = cached(&converted_dir(&documents.home), &prefix)?;
    Some(Copy {
        path,
        kind,
        hit: true,
        millis: 0,
    })
}

pub(crate) fn convert(
    documents: &Documents,
    source: &Source<'_>,
    cancelled: &CancelFlag,
) -> Converted {
    let started = Instant::now();
    let Some(prefix) = cache_key(source) else {
        return Converted::NotADocument;
    };
    let dir = converted_dir(&documents.home);
    if let Some((path, kind)) = cached(&dir, &prefix) {
        return Converted::Markdown(Copy {
            path,
            kind,
            hit: true,
            millis: started.elapsed().as_millis(),
        });
    }
    let converter = (documents.converter)();
    if !converter.python.is_file() {
        return Converted::Unavailable(
            "[the kernel venv that converts documents is not built yet; it builds at session start or on the first ipython call — retry in a moment]"
                .to_owned(),
        );
    }
    let pages = match source.pages.map(parse_pages) {
        Some(Ok(pages)) => pages,
        Some(Err(message)) => return Converted::Refused(message),
        None => Vec::new(),
    };
    let ceiling = if pages.is_empty() {
        INPUT_CEILING
    } else {
        PAGED_CEILING
    };
    let size = source.bytes.len() as u64;
    if size > ceiling {
        return Converted::Refused(format!(
            "{size} bytes is past the {} MiB ceiling for converting a document",
            ceiling / (1024 * 1024)
        ));
    }
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return Converted::Refused(format!("cannot create {}: {error}", dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _private_to_the_user =
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    sweep(&dir);
    static ATTEMPT: AtomicU64 = AtomicU64::new(0);
    let staging = dir.join(format!(
        "{prefix}.{}-{}.tmp",
        std::process::id(),
        ATTEMPT.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = run(
        documents,
        &converter,
        &dir,
        source.path,
        &staging,
        &pages,
        cancelled,
    );
    let converted = match outcome {
        Ok(kind) => keep(&dir, &prefix, &staging, kind, started),
        Err(converted) => converted,
    };
    let _staging_gone_or_never_written = std::fs::remove_file(&staging);
    converted
}

fn keep(dir: &Path, prefix: &str, staging: &Path, kind: String, started: Instant) -> Converted {
    let text = match std::fs::read(staging) {
        Ok(text) => text,
        Err(error) => return Converted::Refused(format!("the converter wrote nothing: {error}")),
    };
    let path = dir.join(format!("{prefix}.{}.{kind}.md", markdown_hash(&text)));
    if let Err(error) = std::fs::rename(staging, &path) {
        return Converted::Refused(format!("cannot keep the copy: {error}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _read_only = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444));
    }
    Converted::Markdown(Copy {
        path,
        kind,
        hit: false,
        millis: started.elapsed().as_millis(),
    })
}

#[cfg(target_os = "linux")]
fn unshare_works() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| {
        crate::process::command("unshare")
            .args(["-Urn", "--", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Seatbelt on macOS (no network, the cache dir as the one writable root), a user and network
/// namespace on Linux where `unshare` allows one; a parser escape needs neither.
fn command_for(
    documents: &Documents,
    converter: &Converter,
    dir: &Path,
    source: &Path,
    staging: &Path,
    pages: &[u64],
) -> std::process::Command {
    let python = converter.python.to_string_lossy().into_owned();
    let mut command = if Sandbox::available() {
        let mut sandbox = Sandbox::for_workspace(dir, &documents.home, None);
        sandbox.writable = vec![dir.to_path_buf()];
        let (program, wrapped) = sandbox.wrap(&python, &[]);
        let mut command = crate::process::command(program);
        command.args(wrapped);
        command
    } else {
        plain_command(converter)
    };
    command.current_dir(dir);
    command.args(["-I", "-c", CONVERT]);
    command.arg(source.as_os_str()).arg(staging.as_os_str());
    command.arg(serde_json::Value::from(pages).to_string());
    command
}

#[cfg(target_os = "linux")]
fn plain_command(converter: &Converter) -> std::process::Command {
    if unshare_works() {
        let mut command = crate::process::command("unshare");
        command.args(["-Urn", "--"]).arg(&converter.python);
        return command;
    }
    crate::process::command(&converter.python)
}

#[cfg(not(target_os = "linux"))]
fn plain_command(converter: &Converter) -> std::process::Command {
    crate::process::command(&converter.python)
}

fn run(
    documents: &Documents,
    converter: &Converter,
    dir: &Path,
    source: &Path,
    staging: &Path,
    pages: &[u64],
    cancelled: &CancelFlag,
) -> Result<String, Converted> {
    let command = command_for(documents, converter, dir, source, staging, pages);
    let deadline = Instant::now() + documents.timeout;
    let caller = Arc::clone(cancelled);
    let stop: CancelFlag = Arc::new(move || caller() || Instant::now() > deadline);
    let capture =
        crate::process::run_captured(command, None, &stop, 8 * 1024).map_err(Converted::Refused)?;
    if capture.cancelled {
        return Err(Converted::Refused(format!(
            "conversion stopped (cancelled, or past {}s)",
            documents.timeout.as_secs()
        )));
    }
    let status = capture
        .stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .unwrap_or_default();
    let text = |key: &str| status.get(key).and_then(serde_json::Value::as_str);
    let count = |key: &str| status.get(key).and_then(serde_json::Value::as_u64);
    if status.get("ok").is_some() {
        return Ok(text("kind").unwrap_or("document").to_owned());
    }
    if status.get("unsupported").is_some() {
        return Err(Converted::NotADocument);
    }
    if let Some(missing) = text("missing") {
        return Err(Converted::Unavailable(format!(
            "[no document converter: the kernel venv lacks anydoc ({missing})]"
        )));
    }
    if let (Some(blank), Some(total)) = (count("ocr"), count("page_count")) {
        let page = pages.first().copied().unwrap_or(1);
        return Err(Converted::Refused(format!(
            "no text layer on any of the {blank} PDF page(s) read of {total} (image-only or vector art), so there is no text to show; to see page {page}, render it to PNG in ipython: `%pip install pymupdf`, then `import pymupdf; p = \"/tmp/page-{page}.png\"; pymupdf.open({source})[{index}].get_pixmap(dpi=200).save(p); print(await attach_image(p))`",
            source = python_str(source),
            index = page.saturating_sub(1),
        )));
    }
    let reason = text("error").map_or_else(
        || {
            let last = capture.stderr.trim().lines().last().unwrap_or("no output");
            format!("the converter failed: {last}")
        },
        str::to_owned,
    );
    Err(Converted::Refused(reason))
}
