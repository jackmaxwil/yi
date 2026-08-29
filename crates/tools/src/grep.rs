use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::builtins::walk_files;
use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output, error_output_kind, require_str,
    resolve_path, text_output,
};

const PAGE_CAP: usize = 200;
const COLLECTION_CAP: usize = 2_000;
const CONTEXT_CAP: usize = 10;
const FILES_PAGE_CAP: usize = 200;
/// Matches the patcher's reveal clip: a clipped row never joins the seen set.
const LINE_CLIP: usize = 512;

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

fn build_matchers(
    input: &Map<String, Value>,
    pattern: &str,
) -> Result<(regex::Regex, Option<globset::GlobMatcher>), Box<ToolOutput>> {
    let invalid = |message: String| Box::new(error_output_kind(message, "invalid_args"));
    let use_regex = input.get("regex").and_then(Value::as_bool).unwrap_or(false);
    let ignore_case = input
        .get("ignore_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source = if use_regex {
        pattern.to_owned()
    } else {
        regex::escape(pattern)
    };
    let matcher = regex::RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
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
    root: &Path,
    matcher: &regex::Regex,
    include: Option<&globset::GlobMatcher>,
) -> Collected {
    let mut files: Vec<FileHits> = Vec::new();
    let mut total = 0_usize;
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
        let normalized = crate::hashline::normalize::normalize_to_lf(
            crate::hashline::normalize::strip_bom(&raw).text,
        );
        let mut hits: Vec<usize> = normalized
            .split('\n')
            .enumerate()
            .filter(|(_, line)| matcher.is_match(line))
            .map(|(index, _)| index)
            .collect();
        if hits.is_empty() {
            return true;
        }
        let room = COLLECTION_CAP.saturating_sub(total);
        if hits.len() >= room {
            hits.truncate(room);
            collection_capped = true;
        }
        total = total.saturating_add(hits.len());
        files.push(FileHits {
            display: path.display().to_string(),
            canonical: path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy()
                .into_owned(),
            normalized,
            hits,
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
    /// shown in full width join the snapshot's seen set so a hit can anchor an
    /// edit without an intervening read; clipped rows never do.
    fn render_file(&self, file: &FileHits, page_hits: &[usize], context: usize) -> Vec<String> {
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
            let lo = index.saturating_sub(context);
            let hi = index.saturating_add(context).min(last);
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
        match &self.hashline {
            // Tagged mode: a `[path#TAG]` header then `N:text` rows the edit
            // tool can anchor on directly.
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
            // Bare mode keeps the classic grep shape: `path:N:text` hits,
            // `path-N-text` context.
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

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents. Hits group per file under a [path#TAG] header with LINE:TEXT rows (context LINE-TEXT); tag+line anchor edits directly. Literal unless regex=true. Pages 200 hits via offset. Multiline searches: rg via bash."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Text to find (literal unless regex=true)"},
                "path": {"type": "string", "description": "Directory or file to search (default: cwd)"},
                "regex": {"type": "boolean", "description": "Rust regex syntax (default literal)"},
                "ignore_case": {"type": "boolean", "description": "Case-insensitive"},
                "include": {"type": "string", "description": "Relative-path glob filter"},
                "type": {"type": "string", "description": "Type filter: rust, py, js, ts, md, toml, json, yaml, sh, html, css, c"},
                "context": {"type": "integer", "description": "Context lines per side (max 10)"},
                "offset": {"type": "integer", "description": "Matches to skip (paging)"},
                "files_with_matches": {"type": "boolean", "description": "Paths only, no content rows"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let pattern = match require_str(&input, "pattern") {
            Ok(pattern) => pattern,
            Err(message) => return error_output(message),
        };
        let files_only = input
            .get("files_with_matches")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (matcher, include) = match build_matchers(&input, pattern) {
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

        let collected = collect(&root, &matcher, include.as_ref());
        let total_label = if collected.collection_capped {
            format!("at least {}", collected.total)
        } else {
            collected.total.to_string()
        };
        let mut rows: Vec<String> = Vec::new();
        let shown;
        if files_only {
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
            for file in &collected.files {
                if taken >= PAGE_CAP {
                    break;
                }
                let skip_here = file.hits.len().min(offset.saturating_sub(skipped));
                skipped = skipped.saturating_add(skip_here);
                let page_hits: Vec<usize> = file
                    .hits
                    .iter()
                    .copied()
                    .skip(skip_here)
                    .take(PAGE_CAP.saturating_sub(taken))
                    .collect();
                if page_hits.is_empty() {
                    continue;
                }
                taken = taken.saturating_add(page_hits.len());
                rows.extend(self.render_file(file, &page_hits, context_lines));
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
            rows.push(format!(
                "[offset {offset} is beyond the {total_label} collected matches]"
            ));
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
