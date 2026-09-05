use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::builtins::walk_files;
use crate::hashline::normalize::{LineEnding, detect_line_ending, restore_line_endings};
use crate::hashline::types::BlockResolverRequest;
use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output_kind, resolve_path, text_output,
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
) -> Result<(regex::Regex, Option<globset::GlobMatcher>), Box<ToolOutput>> {
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
        .map_err(|error| invalid(format!("invalid regex pattern: {error}")))?;
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
            globset::GlobBuilder::new(&source)
                .literal_separator(false)
                .build()
                .map_err(|error| invalid(format!("invalid include glob: {error}")))?
                .compile_matcher(),
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
    ending: LineEnding,
    bom: &'static str,
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
    matcher: &regex::Regex,
    include: Option<&globset::GlobMatcher>,
    options: &Options,
) -> Collected {
    let mut files: Vec<FileHits> = Vec::new();
    let mut total = 0_usize;
    let mut retained = 0_usize;
    let mut collection_capped = false;
    let mut search_file = |path: &Path| -> bool {
        if let Some(include) = include {
            let relative = path.strip_prefix(root).unwrap_or(path);
            if !include.is_match(relative) {
                return true;
            }
        }
        let Ok(bytes) = fs::read(path) else {
            return true;
        };
        if bytes.iter().take(4096).any(|byte| *byte == 0) {
            return true;
        }
        let raw = String::from_utf8_lossy(&bytes);
        let stripped = crate::hashline::normalize::strip_bom(&raw);
        let ending = detect_line_ending(stripped.text);
        let normalized = crate::hashline::normalize::normalize_to_lf(stripped.text);
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
            ending,
            bom: stripped.bom,
        });
        !collection_capped
    };
    if root.is_file() {
        search_file(root);
    } else {
        walk_files(root, &mut search_file);
    }
    Collected {
        files,
        total,
        collection_capped,
    }
}

impl GrepTool {
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
        let collected = collect(root, root, &matcher, None, &options);
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
        for file in &collected.files {
            let after: String = if options.multiline {
                matcher
                    .replace_all(&file.normalized, replacement)
                    .into_owned()
            } else {
                file.normalized
                    .split('\n')
                    .map(|line| matcher.replace_all(line, replacement))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if after == file.normalized {
                continue;
            }
            changed = changed.saturating_add(1);
            let patch = crate::diff::patch(&file.normalized, &after, Path::new(&file.display));
            rows.push(patch.as_str().trim_end().to_owned());
            if !options.apply {
                continue;
            }
            let persisted = format!("{}{}", file.bom, restore_line_endings(&after, file.ending));
            match fs::write(&file.canonical, persisted) {
                Ok(()) => {
                    written = written.saturating_add(1);
                    if let Some(state) = &self.hashline {
                        crate::hashline::tool::record_write_snapshot(
                            state,
                            Path::new(&file.canonical),
                            &after,
                        );
                    }
                }
                Err(error) => failures.push(format!("{}: {error}", file.display)),
            }
        }
        if changed == 0 {
            rows.push("No matches found".to_owned());
        } else if options.apply {
            rows.push(format!("applied to {written} of {changed} files"));
        } else {
            rows.push(format!(
                "[preview: {changed} files would change — pass apply=true to write]"
            ));
        }
        rows.extend(failures.iter().map(|failure| format!("failed: {failure}")));
        let mut output = text_output(rows.join("\n"));
        output.result.details = json!({
            "hits": collected.total,
            "files": collected.files.len(),
            "changed": changed,
            "applied": written,
        });
        output.is_error = !failures.is_empty();
        output
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a regex (literal=true for plain text). Hits group per file under a [path#TAG] header with LINE:TEXT rows; tag+line anchor edits directly. block shows each hit's enclosing function, def keeps definition lines, count counts per file, replace previews a rewrite and apply writes it. Pages 200 hits via offset."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": ["string", "array"], "items": {"type": "string"}, "description": "Regex (Rust syntax), or an array of them matched as alternatives"},
                "path": {"type": "string", "description": "Directory or file to search (default: cwd)"},
                "literal": {"type": "boolean", "description": "Match the pattern as plain text"},
                "ignore_case": {"type": "boolean", "description": "Case-insensitive"},
                "include": {"type": "string", "description": "Relative-path glob filter"},
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
        let (matcher, include) = match build_matchers(&input, &options) {
            Ok(built) => built,
            Err(output) => return *output,
        };
        let context_lines = input
            .get("context")
            .and_then(Value::as_u64)
            .map_or(0, |lines| usize::try_from(lines).unwrap_or(0))
            .min(CONTEXT_CAP);
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .map_or(0, |skip| usize::try_from(skip).unwrap_or(usize::MAX));
        let root = input
            .get("path")
            .and_then(Value::as_str)
            .map_or_else(|| context.cwd.clone(), |path| resolve_path(context, path));

        let collected = collect(&context.cwd, &root, &matcher, include.as_ref(), &options);
        if let Some(replacement) = &options.replace {
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
        if collected.collection_capped {
            rows.push(format!(
                "[match collection stopped at {COLLECTION_CAP} before every file was scanned — narrow with path, include, or type]"
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
