use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::builtins::walk_files;
use crate::hashline::normalize::{Endings, split};
use crate::hashline::types::BlockResolverRequest;
use crate::tool::{
    RootedGlob, Tool, ToolContext, ToolKind, ToolOutput, error_output_kind, resolve_path,
    rooted_glob, text_output,
};

const PAGE_CAP: usize = 200;
const COLLECTION_CAP: usize = 2_000;
/// The hit cap bounds rows, not bytes: one minified file with a single hit
/// still retains its whole text, so the walk also stops on retained bytes.
const COLLECTION_BYTE_CAP: usize = 8 * 1024 * 1024;
const CONTEXT_CAP: usize = 10;
const FILES_PAGE_CAP: usize = 200;
const LINE_CLIP: usize = crate::hashline::patcher::SEEN_LINE_REVEAL_MAX_COLUMNS;
/// Invariant: a grep must not flush the snapshot store's LRU and strand the
/// tag a read just minted, so only the page's first files mint.
const TAG_FILES_CAP: usize = 20;
/// A block per hit is a page of code, not a page of rows.
const BLOCK_PAGE_CAP: usize = 20;
/// A rename past these is a refactor the model should see file by file.
const REPLACE_FILES_CAP: usize = 50;
const REPLACE_HITS_CAP: usize = 500;
/// Queries whose last sweep is remembered; past this the memory starts over.
const SWEEPS_KEPT: usize = 64;

/// `type` shorthand for an include glob; a name outside the map fails loudly.
const TYPES: [(&str, &str); 12] = [
    ("rust", "*.rs"),
    ("py", "*.py"),
    ("js", "*.{js,jsx,mjs,cjs}"),
    ("ts", "*.{ts,tsx}"),
    ("md", "*.md"),
    ("toml", "*.toml"),
    ("json", "*.json"),
    ("yaml", "*.{yaml,yml}"),
    ("sh", "*.sh"),
    ("html", "*.html"),
    ("css", "*.css"),
    ("c", "*.{c,h}"),
];

#[derive(Default)]
pub struct GrepTool {
    pub hashline: Option<crate::hashline::tool::SharedHashline>,
}

fn invalid(message: String) -> Box<ToolOutput> {
    Box::new(error_output_kind(
        message,
        yi_types::event::ToolErrorKind::InvalidArgs,
    ))
}

fn patterns(input: &Map<String, Value>) -> Result<Vec<String>, Box<ToolOutput>> {
    match input.get("pattern") {
        Some(Value::String(one)) if !one.is_empty() => Ok(vec![one.clone()]),
        Some(Value::Array(many)) => {
            let list: Vec<String> = many
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect();
            if list.is_empty() || list.len() != many.len() {
                return Err(invalid(
                    "pattern array must hold non-empty strings".to_owned(),
                ));
            }
            Ok(list)
        }
        Some(_) | None => Err(invalid(
            "pattern is required: a regex, or an array of them".to_owned(),
        )),
    }
}

struct Options {
    literal: bool,
    ignore_case: bool,
    multiline: bool,
    def: bool,
    count: bool,
    block: bool,
    files_only: bool,
    replace: Option<String>,
    apply: bool,
}

fn flag(input: &Map<String, Value>, name: &str) -> bool {
    input.get(name).and_then(Value::as_bool).unwrap_or(false)
}

impl Options {
    fn from(input: &Map<String, Value>) -> Self {
        Self {
            literal: flag(input, "literal"),
            ignore_case: flag(input, "ignore_case"),
            multiline: flag(input, "multiline"),
            def: flag(input, "def"),
            count: flag(input, "count"),
            block: flag(input, "block"),
            files_only: flag(input, "files_with_matches"),
            replace: input
                .get("replace")
                .and_then(Value::as_str)
                .map(str::to_owned),
            apply: flag(input, "apply"),
        }
    }
}

fn build_matchers(
    input: &Map<String, Value>,
    options: &Options,
    root: &Path,
) -> Result<(regex::Regex, Option<RootedGlob>), Box<ToolOutput>> {
    let alternatives: Vec<String> = patterns(input)?
        .into_iter()
        .map(|pattern| {
            if options.literal {
                regex::escape(&pattern)
            } else {
                format!("(?:{pattern})")
            }
        })
        .collect();
    let source = alternatives.join("|");
    let matcher = regex::RegexBuilder::new(&source)
        .case_insensitive(options.ignore_case)
        .multi_line(options.multiline)
        .dot_matches_new_line(options.multiline)
        .build()
        .map_err(|error| {
            let error = error.to_string();
            let unlinked = if error.contains("Unicode property") {
                "\n[… this build links Unicode categories and scripts; Age and the grapheme, word and sentence break properties are not linked]"
            } else {
                ""
            };
            invalid(format!("invalid regex pattern: {error}{unlinked}"))
        })?;
    let include_source = match (
        input.get("include").and_then(Value::as_str),
        input.get("type").and_then(Value::as_str),
    ) {
        (Some(_), Some(_)) => return Err(invalid("pass include or type, not both".to_owned())),
        (Some(glob), None) => Some(glob.to_owned()),
        (None, Some(name)) => match type_glob(name) {
            Some(glob) => Some(format!("**/{glob}")),
            None => {
                let known: Vec<&str> = TYPES.iter().map(|(key, _)| *key).collect();
                return Err(invalid(format!(
                    "unknown type {name:?}; one of: {}",
                    known.join(", ")
                )));
            }
        },
        (None, None) => None,
    };
    let include = match include_source {
        Some(source) => Some(
            rooted_glob(&source, root)
                .map_err(|error| invalid(format!("invalid include glob: {error}")))?,
        ),
        None => None,
    };
    Ok((matcher, include))
}

struct FileHits {
    display: String,
    canonical: String,
    normalized: String,
    hits: Vec<usize>,
    endings: Endings,
    utf8: bool,
}

fn hits_in(matcher: &regex::Regex, normalized: &str, multiline: bool, def: bool) -> Vec<usize> {
    let mut hits: Vec<usize> = if multiline {
        let mut hits: Vec<usize> = matcher
            .find_iter(normalized)
            .map(|found| normalized[..found.start()].matches('\n').count())
            .collect();
        hits.dedup();
        hits
    } else {
        normalized
            .split('\n')
            .enumerate()
            .filter(|(_, line)| matcher.is_match(line))
            .map(|(index, _)| index)
            .collect()
    };
    if def {
        let lines: Vec<&str> = normalized.split('\n').collect();
        hits.retain(|index| {
            lines
                .get(*index)
                .is_some_and(|line| crate::orient::is_decl(line))
        });
    }
    hits
}

struct Collected {
    files: Vec<FileHits>,
    total: usize,
    collection_capped: bool,
    binary_skipped: usize,
    documents_searched: usize,
    documents_unsearched: Vec<String>,
    walled: crate::builtins::Walked,
}

/// A search converts at most this many documents it has no copy of; the rest are counted.
const DOCUMENTS_CONVERTED_PER_SEARCH: usize = 20;
const DOCUMENT_SEARCH_MAX: usize = 16 * 1024 * 1024;

/// Documents are searched through the Markdown `read` shows, so a hit's line number is one
/// `read` can open. Never in replace mode: nothing writes text back into a document.
struct DocumentSearch<'a> {
    documents: crate::Documents,
    cancelled: &'a crate::tool::CancelFlag,
    converted: usize,
}

impl DocumentSearch<'_> {
    /// The Markdown, or why there is none: a refusal's own first clause, or that it was not
    /// converted in this search.
    fn markdown(&mut self, path: &Path, bytes: &[u8]) -> Result<String, String> {
        let source = crate::document::Source {
            path,
            bytes,
            pages: None,
        };
        let copy = match crate::document::peek(&self.documents, &source) {
            Some(copy) => copy,
            None if self.converted < DOCUMENTS_CONVERTED_PER_SEARCH
                && bytes.len() <= DOCUMENT_SEARCH_MAX =>
            {
                self.converted = self.converted.saturating_add(1);
                match crate::document::convert(&self.documents, &source, self.cancelled) {
                    crate::document::Converted::Markdown(copy) => copy,
                    crate::document::Converted::Refused(reason) => {
                        let short = reason.split([';', ',']).next().unwrap_or(&reason);
                        let short = short.split(" (").next().unwrap_or(short);
                        return Err(short.trim().chars().take(80).collect());
                    }
                    crate::document::Converted::Unavailable(_) => {
                        return Err("no converter yet".to_owned());
                    }
                    crate::document::Converted::NotADocument => {
                        return Err("not a document the converter reads".to_owned());
                    }
                }
            }
            None => return Err("not converted yet".to_owned()),
        };
        fs::read_to_string(copy.path).map_err(|error| error.to_string())
    }
}

/// Extensions one language splits its source across, so a header's name is found in its code.
const LANGUAGES: [&[&str]; 4] = [
    &["c", "h"],
    &["cc", "cpp", "cxx", "hpp", "hh", "h"],
    &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
    &["py", "pyi"],
];

fn same_language(extension: &str) -> String {
    let mut family: Vec<&str> = (LANGUAGES.iter())
        .filter(|family| family.contains(&extension))
        .flat_map(|family| family.iter().copied())
        .collect();
    family.sort_unstable();
    family.dedup();
    match family.as_slice() {
        [] => format!("**/*.{extension}"),
        extensions => format!("**/*.{{{}}}", extensions.join(",")),
    }
}

fn type_glob(name: &str) -> Option<&'static str> {
    TYPES
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, glob)| *glob)
}

fn clip(line: &str) -> (String, bool) {
    if line.chars().count() > LINE_CLIP {
        let clipped: String = line.chars().take(LINE_CLIP).collect();
        (format!("{clipped}\u{2026}"), true)
    } else {
        (line.to_owned(), false)
    }
}

fn collect(
    cwd: &Path,
    root: &Path,
    deny: &[PathBuf],
    matcher: &regex::Regex,
    include: Option<&RootedGlob>,
    options: &Options,
    mut documents: Option<DocumentSearch<'_>>,
) -> Collected {
    let mut files: Vec<FileHits> = Vec::new();
    let mut total = 0_usize;
    let mut retained = 0_usize;
    let mut collection_capped = false;
    let mut binary_skipped = 0_usize;
    let mut documents_searched = 0_usize;
    let mut documents_unsearched: Vec<String> = Vec::new();
    let mut search_file = |path: &Path| -> bool {
        if include.is_some_and(|include| !include.matches(path)) {
            return true;
        }
        let Ok(bytes) = fs::read(path) else {
            return true;
        };
        let converted = match documents.as_mut() {
            Some(search) if crate::document::could_be_document(&bytes) => {
                match search.markdown(path, &bytes) {
                    Ok(markdown) => {
                        documents_searched = documents_searched.saturating_add(1);
                        Some(markdown)
                    }
                    Err(why) => {
                        let name = path.strip_prefix(cwd).unwrap_or(path).display();
                        documents_unsearched.push(format!("{name} ({why})"));
                        return true;
                    }
                }
            }
            _ => None,
        };
        if converted.is_none() && crate::document::has_nul(&bytes) {
            binary_skipped = binary_skipped.saturating_add(1);
            return true;
        }
        let (raw, utf8) = match converted {
            Some(markdown) => (markdown, true),
            None => match String::from_utf8(bytes) {
                Ok(text) => (text, true),
                Err(error) => (
                    String::from_utf8_lossy(error.as_bytes()).into_owned(),
                    false,
                ),
            },
        };
        let (normalized, endings) = split(&raw);
        let mut hits = hits_in(matcher, &normalized, options.multiline, options.def);
        if hits.is_empty() {
            return true;
        }
        let room = COLLECTION_CAP.saturating_sub(total);
        if room == 0 {
            collection_capped = true;
            return false;
        }
        if hits.len() > room {
            hits.truncate(room);
            collection_capped = true;
        }
        total = total.saturating_add(hits.len());
        retained = retained.saturating_add(normalized.len());
        if retained >= COLLECTION_BYTE_CAP {
            collection_capped = true;
        }
        files.push(FileHits {
            display: path.strip_prefix(cwd).unwrap_or(path).display().to_string(),
            canonical: path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy()
                .into_owned(),
            normalized,
            hits,
            endings,
            utf8,
        });
        !collection_capped
    };
    let walled = if root.is_file() {
        search_file(root);
        crate::builtins::Walked::default()
    } else {
        walk_files(root, deny, &mut search_file)
    };
    Collected {
        files,
        total,
        collection_capped,
        binary_skipped,
        documents_searched,
        documents_unsearched,
        walled,
    }
}

impl GrepTool {
    /// Remembers each query's last sweep; a later page over a different one says so, because its
    /// offset now counts matches the model has not paged through.
    fn sweep_row(
        &self,
        input: &Map<String, Value>,
        offset: usize,
        collected: &Collected,
    ) -> Option<String> {
        let state = self.hashline.as_ref()?;
        let query: std::collections::BTreeMap<&str, &Value> = input
            .iter()
            .filter(|(key, _)| key.as_str() != "offset")
            .map(|(key, value)| (key.as_str(), value))
            .collect();
        let key = serde_json::to_string(&query).ok()?;
        let mut bytes: Vec<u8> = Vec::new();
        for file in &collected.files {
            let width = u64::try_from(file.display.len()).unwrap_or(u64::MAX);
            let hits = u64::try_from(file.hits.len()).unwrap_or(u64::MAX);
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(file.display.as_bytes());
            bytes.extend_from_slice(&hits.to_le_bytes());
        }
        let print = u64::from(xxhash_rust::xxh32::xxh32(&bytes, 0)) << 32
            | u64::from(xxhash_rust::xxh32::xxh32(&bytes, 1));
        let mut guard = crate::hashline::tool::lock_state(state);
        let sweeps = &mut guard.grep_sweeps;
        if sweeps.len() >= SWEEPS_KEPT && !sweeps.contains_key(&key) {
            sweeps.clear();
        }
        let kept = sweeps.insert(key, print);
        (offset > 0 && kept.is_some_and(|kept| kept != print)).then(|| {
            "[matches changed since the last page; offsets now count the current sweep]".to_owned()
        })
    }

    /// Windows around each page hit, deduped forward like `rg` output. Rows
    /// shown in full width join the seen set so a hit anchors an edit.
    fn render_file(
        &self,
        file: &FileHits,
        page_hits: &[usize],
        context: usize,
        mint: bool,
        block: bool,
    ) -> Vec<String> {
        enum Row {
            Gap,
            Note(String),
            Line {
                number: u64,
                hit: bool,
                text: String,
            },
        }
        let lines: Vec<&str> = file.normalized.split('\n').collect();
        let last = lines.len().saturating_sub(1);
        let mut collected: Vec<Row> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut emitted: Option<usize> = None;
        for &index in page_hits {
            let span = block
                .then(|| {
                    crate::hashline::patcher::block_resolver(&BlockResolverRequest {
                        path: &file.display,
                        text: &file.normalized,
                        line: u64::try_from(index).ok()?.checked_add(1)?,
                    })
                })
                .flatten()
                .and_then(|span| {
                    Some((
                        usize::try_from(span.start).ok()?.checked_sub(1)?,
                        usize::try_from(span.end).ok()?.checked_sub(1)?,
                    ))
                });
            if block && span.is_none() {
                collected.push(Row::Note(format!(
                    "[line {} opens no block; context shown instead]",
                    index.saturating_add(1)
                )));
            }
            let (lo, hi) =
                span.unwrap_or((index.saturating_sub(context), index.saturating_add(context)));
            let hi = hi.min(last);
            let start = match emitted {
                Some(end) if lo <= end => end,
                Some(_) => {
                    collected.push(Row::Gap);
                    lo
                }
                None => lo,
            };
            for row in start..=hi {
                let Some(text) = lines.get(row) else {
                    continue;
                };
                let number = row.saturating_add(1) as u64;
                let (shown, clipped) = clip(text);
                if !clipped {
                    seen.push(number);
                }
                collected.push(Row::Line {
                    number,
                    hit: page_hits.binary_search(&row).is_ok(),
                    text: shown,
                });
            }
            emitted = Some(hi.saturating_add(1));
        }
        match self.hashline.as_ref().filter(|_| mint) {
            // A tagged file anchors an edit directly; a bare one names itself
            // on every row and anchors nothing.
            Some(state) => {
                let tag = crate::hashline::tool::lock_state(state).snapshots.record(
                    &file.canonical,
                    &file.normalized,
                    Some(&seen),
                );
                let mut rows = vec![format!("[{}#{tag}]", file.display)];
                rows.extend(collected.iter().map(|row| match row {
                    Row::Gap => "--".to_owned(),
                    Row::Note(note) => note.clone(),
                    Row::Line { number, hit, text } => {
                        let sep = if *hit { ':' } else { '-' };
                        format!("{number}{sep}{text}")
                    }
                }));
                rows
            }
            None => collected
                .iter()
                .map(|row| match row {
                    Row::Gap => "--".to_owned(),
                    Row::Note(note) => note.clone(),
                    Row::Line { number, hit, text } => {
                        let sep = if *hit { ':' } else { '-' };
                        format!("{}{sep}{number}{sep}{text}", file.display)
                    }
                })
                .collect(),
        }
    }
}

impl GrepTool {
    /// Every whole-word mention of `identifier` under `root` except the line at `skip`,
    /// rendered like a grep page so each row anchors an edit.
    pub(crate) fn references(
        &self,
        root: &Path,
        deny: &[PathBuf],
        identifier: &str,
        skip: Option<(&str, usize)>,
        cap: usize,
    ) -> (Vec<String>, usize) {
        let source = format!(r"(?-u:\b){}(?-u:\b)", regex::escape(identifier));
        let Ok(matcher) = regex::Regex::new(&source) else {
            return (Vec::new(), 0);
        };
        let options = Options {
            literal: false,
            ignore_case: false,
            multiline: false,
            def: false,
            count: false,
            block: false,
            files_only: false,
            replace: None,
            apply: false,
        };
        let kind = skip.and_then(|(path, _)| Path::new(path).extension()?.to_str());
        let same_kind = kind.and_then(|kind| rooted_glob(&same_language(kind), root).ok());
        let collected = collect(
            root,
            root,
            deny,
            &matcher,
            same_kind.as_ref(),
            &options,
            None,
        );
        let mut rows: Vec<String> = Vec::new();
        let mut taken = 0_usize;
        let mut total = 0_usize;
        for file in &collected.files {
            let hits: Vec<usize> = file
                .hits
                .iter()
                .copied()
                .filter(|index| skip != Some((file.canonical.as_str(), *index)))
                .collect();
            total = total.saturating_add(hits.len());
            if taken >= cap || hits.is_empty() {
                continue;
            }
            let page: Vec<usize> = hits.into_iter().take(cap.saturating_sub(taken)).collect();
            taken = taken.saturating_add(page.len());
            rows.extend(self.render_file(file, &page, 0, true, false));
        }
        rows.extend(collected.walled.notices());
        (rows, total)
    }

    /// The diff every hit would produce, written only when `apply` is set; a request past the
    /// caps writes nothing at all.
    fn replace(
        &self,
        collected: &Collected,
        matcher: &regex::Regex,
        replacement: &str,
        options: &Options,
    ) -> ToolOutput {
        if collected.files.len() > REPLACE_FILES_CAP || collected.total > REPLACE_HITS_CAP {
            return *invalid(format!(
                "replace would touch {} files / {} hits; the caps are {REPLACE_FILES_CAP} / {REPLACE_HITS_CAP} — narrow with path, include or type",
                collected.files.len(),
                collected.total
            ));
        }
        let mut rows: Vec<String> = Vec::new();
        let mut changed = 0_usize;
        let mut written = 0_usize;
        let mut failures: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut skipped_hits = 0_usize;
        for file in &collected.files {
            if !file.utf8 {
                skipped.push(file.display.clone());
                skipped_hits = skipped_hits.saturating_add(file.hits.len());
                continue;
            }
            let (after, origins) =
                replaced(matcher, &file.normalized, replacement, options.multiline);
            if after == file.normalized {
                continue;
            }
            changed = changed.saturating_add(1);
            let patch = crate::diff::patch(&file.normalized, &after, Path::new(&file.display));
            rows.push(patch.as_str().trim_end().to_owned());
            if !options.apply {
                continue;
            }
            let persisted = file.endings.restore(&after, &origins);
            match fs::write(&file.canonical, persisted) {
                Ok(()) => {
                    written = written.saturating_add(1);
                    if let Some(state) = &self.hashline {
                        crate::hashline::tool::record_view_snapshot(
                            state,
                            Path::new(&file.canonical),
                            &after,
                            &crate::diff::shown_after_lines(&file.normalized, &after),
                        );
                    }
                }
                Err(error) => failures.push(format!("{}: {error}", file.display)),
            }
        }
        if changed == 0 {
            rows.push(if collected.total == 0 {
                "No matches found".to_owned()
            } else if skipped.is_empty() {
                format!(
                    "nothing changed: each of the {} matches is replaced by itself",
                    collected.total
                )
            } else {
                format!(
                    "nothing written: {skipped_hits} of {} matches in files skipped (not UTF-8)",
                    collected.total
                )
            });
        } else if options.apply {
            rows.push(format!("applied to {written} of {changed} files"));
        } else {
            rows.push(format!(
                "[preview: {changed} files would change — pass apply=true to write]"
            ));
        }
        rows.extend(failures.iter().map(|failure| format!("failed: {failure}")));
        rows.extend(
            skipped
                .iter()
                .map(|path| format!("skipped (not UTF-8): {path}")),
        );
        rows.extend(collected.walled.notices());
        let mut output = text_output(rows.join("\n"));
        output.result.details = json!({
            "hits": collected.total,
            "files": collected.files.len(),
            "changed": changed,
            "applied": written,
            "skipped": skipped.len(),
        });
        output.is_error = !failures.is_empty();
        output
    }
}

/// Every match expanded, with each output `\n`'s source line; a per-line join is its line's, matched or not.
fn replaced(
    matcher: &regex::Regex,
    text: &str,
    replacement: &str,
    multiline: bool,
) -> (String, Vec<Option<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut origins = Vec::new();
    if multiline {
        substitute(matcher, text, replacement, 0, &mut out, &mut origins);
    } else {
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
                origins.push(index.checked_sub(1));
            }
            substitute(matcher, line, replacement, index, &mut out, &mut origins);
        }
    }
    (out, origins)
}

/// `text`'s matches expanded onto `out` from source line `first`; a `\n` inside a match is the replacement's, `None`.
fn substitute(
    matcher: &regex::Regex,
    text: &str,
    replacement: &str,
    first: usize,
    out: &mut String,
    origins: &mut Vec<Option<usize>>,
) {
    let mut line = first;
    let mut cursor = 0_usize;
    let mut matches = matcher.captures_iter(text);
    loop {
        let caps = matches.next();
        let found = caps.as_ref().and_then(|caps| caps.get(0));
        let gap = text
            .get(cursor..found.map_or(text.len(), |found| found.start()))
            .unwrap_or_default();
        out.push_str(gap);
        let kept = gap.matches('\n').count();
        origins.extend((line..line.saturating_add(kept)).map(Some));
        line = line.saturating_add(kept);
        let (Some(caps), Some(found)) = (caps.as_ref(), found) else {
            return;
        };
        let start = out.len();
        caps.expand(replacement, out);
        let wrote = out.get(start..).unwrap_or_default().matches('\n').count();
        origins.resize(origins.len().saturating_add(wrote), None);
        line = line.saturating_add(found.as_str().matches('\n').count());
        cursor = found.end();
    }
}

fn count_arg(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|count| usize::try_from(count).ok())
}

/// (context asked, context ignored as written, offset, root). A missing `path` was a clean
/// "No matches found" and a negative `offset` paged from 0: both read as a fine request.
fn page_args(
    input: &Map<String, Value>,
    context: &ToolContext,
) -> Result<(usize, Option<String>, usize, PathBuf), Box<ToolOutput>> {
    let (context_asked, context_ignored) = match input.get("context") {
        None => (0, None),
        Some(value) => match count_arg(value) {
            Some(lines) => (lines, None),
            None => (0, Some(value.to_string())),
        },
    };
    let offset = match input.get("offset").map(|value| (value, count_arg(value))) {
        None => 0,
        Some((_, Some(skip))) => skip,
        Some((value, None)) => {
            return Err(invalid(format!(
                "offset must be a non-negative integer, got {value}"
            )));
        }
    };
    let root = match input.get("path").and_then(Value::as_str) {
        None => context.cwd.clone(),
        Some(path) => {
            let root = resolve_path(context, path);
            if !root.exists() {
                return Err(invalid(format!(
                    "path {path} does not exist ({})",
                    root.display()
                )));
            }
            root
        }
    };
    Ok((context_asked, context_ignored, offset, root))
}

fn walled_write(
    collected: &Collected,
    options: &Options,
    context: &ToolContext,
) -> Option<ToolOutput> {
    if !options.apply {
        return None;
    }
    let file = collected.files.iter().find(|file| {
        crate::builtins::walled(&context.deny_write, &context.cwd.join(&file.display))
    })?;
    Some(error_output_kind(
        crate::builtins::wall_refusal("grep", &file.display, "deny_write"),
        yi_types::event::ToolErrorKind::Denied,
    ))
}

fn cut_notices(collected: &Collected, context_asked: usize, rows: &mut Vec<String>) {
    rows.extend(collected.walled.notices());
    if collected.collection_capped {
        rows.push(format!(
            "[match collection stopped at {COLLECTION_CAP} before every file was scanned — narrow with path, include, or type]"
        ));
    }
    if context_asked > CONTEXT_CAP {
        rows.push(format!(
            "[context clamped to {CONTEXT_CAP} lines per side; asked {context_asked}]"
        ));
    }
    if collected.documents_searched > 0 {
        rows.push(format!(
            "[{} document(s) searched through the Markdown read shows; line numbers are read's]",
            collected.documents_searched
        ));
    }
    if !collected.documents_unsearched.is_empty() {
        let named: Vec<&str> = collected
            .documents_unsearched
            .iter()
            .take(3)
            .map(String::as_str)
            .collect();
        rows.push(format!(
            "[{} document(s) not searched: {}{} — a search converts {DOCUMENTS_CONVERTED_PER_SEARCH}; read one to convert it]",
            collected.documents_unsearched.len(),
            named.join("; "),
            if collected.documents_unsearched.len() > 3 { "; …" } else { "" }
        ));
    }
    if collected.binary_skipped > 0 {
        rows.push(format!(
            "[{} binary files skipped (not text, not a document) — bash: rg -a for those]",
            collected.binary_skipped
        ));
    }
}

impl GrepTool {
    fn document_search<'a>(
        &self,
        options: &Options,
        context: &'a ToolContext,
    ) -> Option<DocumentSearch<'a>> {
        if options.replace.is_some() {
            return None;
        }
        let documents = crate::hashline::tool::documents(self.hashline.as_ref()?)?;
        Some(DocumentSearch {
            documents,
            cancelled: &context.cancelled,
            converted: 0,
        })
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a regex (literal=true for plain text); office documents and PDFs are searched through the Markdown read shows. Hits group per file under a [path#TAG] header with LINE:TEXT rows; tag+line anchor edits directly. block shows each hit's enclosing function, def keeps definition lines, count counts per file, replace previews a rewrite and apply writes it. Pages 200 hits via offset."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": ["string", "array"], "items": {"type": "string"}, "description": "Regex (Rust syntax), or an array of them matched as alternatives"},
                "path": {"type": "string", "description": "Directory or file to search (default: cwd)"},
                "literal": {"type": "boolean", "description": "Match the pattern as plain text"},
                "ignore_case": {"type": "boolean", "description": "Case-insensitive"},
                "include": {"type": "string", "description": "Glob on file paths: relative to path, or absolute"},
                "type": {"type": "string", "description": "Type filter: rust, py, js, ts, md, toml, json, yaml, sh, html, css, c"},
                "context": {"type": "integer", "description": "Context lines per side (max 10)"},
                "block": {"type": "boolean", "description": "Show each hit's enclosing block instead of context lines (max 20 hits per page)"},
                "def": {"type": "boolean", "description": "Only hits on definition lines (fn, struct, class, def, …)"},
                "count": {"type": "boolean", "description": "Per-file hit counts, descending"},
                "multiline": {"type": "boolean", "description": "Match across lines; a hit reports its first line"},
                "replace": {"type": "string", "description": "Replacement text ($1 captures); previews the diff, writes nothing"},
                "apply": {"type": "boolean", "description": "With replace: write the previewed rewrite and tag every file"},
                "offset": {"type": "integer", "description": "Matches to skip (paging)"},
                "files_with_matches": {"type": "boolean", "description": "Paths only, no content rows"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn kind_for(&self, input: &Map<String, Value>) -> ToolKind {
        if input.get("replace").is_some() && flag(input, "apply") {
            ToolKind::Write
        } else {
            ToolKind::Read
        }
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let options = Options::from(&input);
        let files_only = options.files_only;
        let (context_asked, context_ignored, offset, root) = match page_args(&input, context) {
            Ok(args) => args,
            Err(output) => return *output,
        };
        let (matcher, include) = match build_matchers(&input, &options, &root) {
            Ok(built) => built,
            Err(output) => return *output,
        };
        let context_lines = context_asked.min(CONTEXT_CAP);

        let collected = collect(
            &context.cwd,
            &root,
            &context.deny_read,
            &matcher,
            include.as_ref(),
            &options,
            self.document_search(&options, context),
        );
        if let Some(replacement) = &options.replace {
            if let Some(refused) = walled_write(&collected, &options, context) {
                return refused;
            }
            return self.replace(&collected, &matcher, replacement, &options);
        }
        let page_cap = if options.block {
            BLOCK_PAGE_CAP
        } else {
            PAGE_CAP
        };
        let total_label = if collected.collection_capped {
            format!("at least {}", collected.total)
        } else {
            collected.total.to_string()
        };
        let mut rows: Vec<String> = Vec::new();
        let shown;
        if options.count {
            let mut counts: Vec<(usize, &str)> = collected
                .files
                .iter()
                .map(|file| (file.hits.len(), file.display.as_str()))
                .collect();
            counts.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
            let page: Vec<&(usize, &str)> =
                counts.iter().skip(offset).take(FILES_PAGE_CAP).collect();
            shown = page.len();
            rows.extend(page.iter().map(|(count, path)| format!("{count} {path}")));
            if offset.saturating_add(shown) < counts.len() {
                rows.push(format!(
                    "[showing files {}-{} of {} — continue with offset={}]",
                    offset.saturating_add(1),
                    offset.saturating_add(shown),
                    counts.len(),
                    offset.saturating_add(shown)
                ));
            }
        } else if files_only {
            let paths: Vec<&str> = collected
                .files
                .iter()
                .map(|file| file.display.as_str())
                .skip(offset)
                .take(FILES_PAGE_CAP)
                .collect();
            shown = paths.len();
            rows.extend(paths.iter().map(|path| (*path).to_owned()));
            if offset.saturating_add(shown) < collected.files.len() {
                rows.push(format!(
                    "[showing files {}-{} of {} — continue with offset={}]",
                    offset + 1,
                    offset + shown,
                    collected.files.len(),
                    offset + shown
                ));
            }
        } else {
            // The page is a window over the flat match sequence in walk order;
            // each file renders only the hits that fall inside it.
            let mut skipped = 0_usize;
            let mut taken = 0_usize;
            let mut tagged = 0_usize;
            for file in &collected.files {
                if taken >= page_cap {
                    break;
                }
                let skip_here = file.hits.len().min(offset.saturating_sub(skipped));
                skipped = skipped.saturating_add(skip_here);
                let page_hits: Vec<usize> = file
                    .hits
                    .iter()
                    .copied()
                    .skip(skip_here)
                    .take(page_cap.saturating_sub(taken))
                    .collect();
                if page_hits.is_empty() {
                    continue;
                }
                taken = taken.saturating_add(page_hits.len());
                let mint = tagged < TAG_FILES_CAP;
                if tagged == TAG_FILES_CAP {
                    rows.push(format!(
                        "[tags minted for the first {TAG_FILES_CAP} files of this page; the rows below name their file and anchor nothing — page with offset to tag them]"
                    ));
                }
                tagged = tagged.saturating_add(1);
                rows.extend(self.render_file(file, &page_hits, context_lines, mint, options.block));
            }
            shown = taken;
            if shown > 0 && offset.saturating_add(shown) < collected.total {
                rows.push(format!(
                    "[showing matches {}-{} of {total_label} — continue with offset={}]",
                    offset + 1,
                    offset + shown,
                    offset + shown
                ));
            }
        }
        rows.extend(self.sweep_row(&input, offset, &collected));
        cut_notices(&collected, context_asked, &mut rows);
        if let Some(asked) = context_ignored {
            rows.push(format!(
                "[context ignored: asked {asked}, not a non-negative integer; 0 used]"
            ));
        }
        if collected.total == 0 {
            rows.push("No matches found".to_owned());
        } else if shown == 0 {
            let (count, noun) = if files_only || options.count {
                (collected.files.len().to_string(), "matching files")
            } else {
                (total_label, "collected matches")
            };
            rows.push(format!("[offset {offset} is beyond the {count} {noun}]"));
        }
        let mut output = text_output(rows.join("\n"));
        output.result.details = json!({
            "hits": collected.total,
            "shown": shown,
            "offset": offset,
            "files": collected.files.len(),
            "collectionCapped": collected.collection_capped,
        });
        output
    }
}
