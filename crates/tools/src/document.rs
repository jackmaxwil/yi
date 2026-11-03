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
/// Invariant: the converter rides in the binary, so a venv whose `yi` package predates this
/// binary still converts; resolving it as `-m yi.convert` there would fail on a missing module.
const CONVERT: &str = include_str!("../../../python/yi_runtime/src/yi/convert.py");

/// The venv python and the formats its wheel reported when it was built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Converter {
    pub python: PathBuf,
    pub formats: Vec<String>,
}

#[derive(Clone)]
pub struct Documents {
    pub home: PathBuf,
    /// Asked on every read, so a venv built mid-session converts; the description asks once.
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
        || is_pdf(bytes)
}

/// Only a PDF has pages the converter can pick; every other format converts whole.
pub(crate) fn is_pdf(bytes: &[u8]) -> bool {
    bytes
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
        documents, &converter, &dir, source, &staging, &pages, cancelled,
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
    // Linux keeps `unshare`: Landlock leaves the network open and a parser needs none.
    let mut command = if cfg!(target_os = "macos") && Sandbox::available() {
        let mut sandbox = Sandbox::for_workspace(dir, &documents.home, None);
        sandbox.writable = vec![dir.to_path_buf()];
        sandbox.deny_write = Vec::new();
        let (program, wrapped) = sandbox.wrap_offline(&python, &[]);
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
    source: &Source<'_>,
    staging: &Path,
    pages: &[u64],
    cancelled: &CancelFlag,
) -> Result<String, Converted> {
    let command = command_for(documents, converter, dir, source.path, staging, pages);
    let deadline = Instant::now() + documents.timeout;
    let caller = Arc::clone(cancelled);
    let stop: CancelFlag = Arc::new(move || caller() || Instant::now() > deadline);
    // The bytes the read gate judged, never the name again (#890); the name gives the extension.
    let bytes = Some(source.bytes.to_vec());
    let capture = crate::process::run_captured(command, bytes, &stop, 8 * 1024)
        .map_err(Converted::Refused)?;
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
            "no text layer on any of the {blank} PDF page(s) read of {total} (image-only or vector art), so there is no text to show; to see page {page}, render it to PNG in ipython, if its kernel has pymupdf: `import pymupdf; p = \"/tmp/page-{page}.png\"; pymupdf.open({source})[{index}].get_pixmap(dpi=200).save(p); print(await attach_image(p))`",
            source = python_str(source.path),
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

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use std::path::{Path, PathBuf};

    /// Review of #925 (F2): the converter parses untrusted files, so its profile keeps no network,
    /// not even the loopback every other contained spawn has.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_converter_runs_with_no_network() {
        if !super::Sandbox::available() {
            return;
        }
        let converter = super::Converter {
            python: PathBuf::from("/usr/bin/python3"),
            formats: Vec::new(),
        };
        let documents = super::Documents::fixed(PathBuf::from("/home/u"), converter.clone());
        let dir = Path::new("/work/cache");
        let command = super::command_for(&documents, &converter, dir, dir, dir, &[]);
        let args: Vec<String> = (command.get_args())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let policy = args.iter().find(|arg| arg.contains("(deny default)"));
        assert!(
            policy.is_some_and(|policy| !policy.contains("network")),
            "{args:?}"
        );
    }

    /// A venv still carrying an older `yi` package is the state this program must survive, so it
    /// is run by an interpreter that has neither the package nor its extras.
    #[test]
    fn the_carried_converter_program_answers_without_the_kernel_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = match crate::process::command("python3")
            .args([
                "-I",
                "-c",
                super::CONVERT,
                "missing.docx",
                "/dev/null",
                "[]",
            ])
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let reply: serde_json::Value =
            serde_json::from_str(stdout.lines().last().unwrap_or_default())?;
        assert!(
            reply.get("missing").is_some() || reply.get("unsupported").is_some(),
            "{stdout}"
        );
        Ok(())
    }
}
