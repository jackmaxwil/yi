use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};
use yi_types::kernel::{JSON_MIME, RESULT_MIMES};

/// An edit's patch names its file absolutely, while git and a model's read may name it from
/// the repository root: one file either way.
fn same_file(one: &str, other: &str) -> bool {
    let (one, other) = (std::path::Path::new(one), std::path::Path::new(other));
    one.ends_with(other) || other.ends_with(one)
}

pub fn review_files(
    diff: &crate::model::SessionDiff,
    scope: crate::model::ReviewScope,
) -> Vec<(String, String)> {
    use crate::model::ReviewScope;
    match (scope, &diff.branch) {
        (ReviewScope::Branch, Some(branch)) => {
            let mut patches: Vec<(String, String)> = Vec::new();
            for section in branch.patch.split("diff --git ").skip(1) {
                let path = section
                    .lines()
                    .find_map(|line| line.strip_prefix("+++ b/"))
                    .or_else(|| section.lines().find_map(|line| line.strip_prefix("--- a/")))
                    .unwrap_or_default();
                patches.push((path.to_owned(), format!("diff --git {section}")));
            }
            branch
                .files
                .iter()
                .map(|(path, _, _)| {
                    let patch = patches
                        .iter()
                        .find(|(known, _)| known == path)
                        .map(|(_, patch)| patch.clone())
                        .unwrap_or_default();
                    (path.clone(), patch)
                })
                .collect()
        }
        _ => diff
            .files
            .iter()
            .filter(|(_, file)| scope != ReviewScope::Turn || file.turn == diff.turn)
            .map(|(path, file)| (path.clone(), file.patch.clone()))
            .collect(),
    }
}

pub fn hunk_lines(patch: &str) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut at: Option<u32> = None;
    let mut found = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            at = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|part| part.split(',').next())
                .and_then(|start| start.parse().ok());
            found = false;
            continue;
        }
        let Some(current) = at else { continue };
        match line.chars().next() {
            Some('+') if !found => {
                lines.push(current);
                found = true;
                at = Some(current.saturating_add(1));
            }
            Some('+' | ' ') => at = Some(current.saturating_add(1)),
            _ => {}
        }
    }
    lines
}

pub fn review_view(
    diff: Option<&crate::model::SessionDiff>,
    scope: crate::model::ReviewScope,
    note: &str,
    width: usize,
    theme: &Theme,
) -> (String, Vec<Line<'static>>) {
    use crate::model::ReviewScope;
    let unbased = scope == ReviewScope::Branch && diff.is_none_or(|diff| diff.branch.is_none());
    let scope = if unbased { ReviewScope::Session } else { scope };
    let serving = |path: &str| {
        diff.and_then(|diff| diff.files.iter().find(|(known, _)| same_file(known, path)))
            .and_then(|(_, file)| file.serving.clone())
    };
    let (files, patch, base): (Vec<(String, u64, u64)>, String, String) = match (scope, diff) {
        (ReviewScope::Branch, Some(diff)) => diff.branch.as_ref().map_or_else(
            || (Vec::new(), String::new(), String::new()),
            |branch| {
                let loose = if branch.untracked == 0 {
                    String::new()
                } else {
                    format!(" · {} untracked", branch.untracked)
                };
                let base = format!("branch vs base {}{loose}", branch.base);
                (branch.files.clone(), branch.patch.clone(), base)
            },
        ),
        (_, Some(diff)) => {
            let chosen: Vec<&(String, crate::model::FileDiff)> = diff
                .files
                .iter()
                .filter(|(_, file)| scope == ReviewScope::Session || file.turn == diff.turn)
                .collect();
            (
                chosen
                    .iter()
                    .map(|(path, file)| (path.clone(), file.added, file.removed))
                    .collect(),
                chosen
                    .iter()
                    .map(|(_, file)| file.patch.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                match chosen.iter().filter(|(_, file)| !file.tracked).count() {
                    0 => String::new(),
                    loose => format!(" · {loose} untracked"),
                },
            )
        }
        (_, None) => (Vec::new(), String::new(), String::new()),
    };
    let (added, removed) = files.iter().fold((0_u64, 0_u64), |(a, r), (_, add, rem)| {
        (a.saturating_add(*add), r.saturating_add(*rem))
    });
    let noun = if files.len() == 1 { "file" } else { "files" };
    let title = format!(
        "Review · {} · {} {noun} +{added} −{removed}{note}",
        scope.label(),
        files.len()
    );
    let base = match (unbased, scope) {
        (true, _) => format!("no branch diff from git, so this session's edits{base}"),
        (false, ReviewScope::Branch) => base,
        (false, ReviewScope::Turn) => format!("the last turn's edits{base}"),
        (false, ReviewScope::Session) => format!("this session's edits{base}"),
    };
    let mut lines: Vec<Line<'static>> = vec![Line::styled(base, theme.dim_style())];
    if let Some(title) = diff.and_then(|diff| diff.land_armed.as_deref()) {
        lines.push(Line::styled(
            format!("l again to /land {title} · any other key cancels"),
            Style::default().fg(theme.warning),
        ));
    }
    let selected = diff.map_or(0, |diff| diff.selected.min(files.len().saturating_sub(1)));
    for (index, (path, add, rem)) in files.iter().enumerate() {
        let todo = serving(path).map_or_else(String::new, |label| format!("  · {label}"));
        let mark = if index == selected { "▸" } else { "●" };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} {path}"), Style::default().fg(theme.text)),
            Span::styled(format!("  +{add} −{rem}"), theme.dim_style()),
            Span::styled(todo, theme.muted_style()),
        ]));
        let chain = diff
            .and_then(|diff| diff.why.get(path))
            .filter(|_| index == selected);
        for row in chain.into_iter().flatten() {
            lines.push(Line::styled(row.clone(), theme.muted_style()));
        }
    }
    let read_only = diff.map_or(0, |diff| {
        diff.reads
            .keys()
            .filter(|path| !files.iter().any(|(changed, _, _)| same_file(path, changed)))
            .count()
    });
    if read_only > 0 {
        let noun = if read_only == 1 { "file" } else { "files" };
        lines.push(Line::styled(
            format!("○ {read_only} {noun} read only"),
            theme.dim_style(),
        ));
    }
    let dropped = diff
        .filter(|_| scope != ReviewScope::Branch)
        .map_or(0, |diff| {
            let turn_only = scope == ReviewScope::Turn;
            (diff.dropped_at_turn.values())
                .filter(|turn| !turn_only || **turn == diff.turn)
                .count()
        });
    if dropped > 0 {
        let noun = if dropped == 1 { "file" } else { "files" };
        let cap = format!(
            "review keeps {} files, {} MiB",
            crate::model::MAX_DIFF_FILES,
            crate::model::MAX_DIFF_BYTES / (1 << 20)
        );
        lines.push(cap_row(
            &format!("{dropped} older {noun} not shown"),
            &cap,
            "git diff lists them",
            theme,
        ));
    }
    if files.is_empty() {
        lines.push(Line::styled(
            "nothing changed in this scope",
            theme.dim_style(),
        ));
    }
    lines.push(Line::styled(
        "s scope · ↑/↓ file · w why each hunk · l land under the session's title",
        theme.dim_style(),
    ));
    lines.push(Line::default());
    lines.extend(diffview::render(&patch, width, theme, DiffBudget::FULL));
    (title, lines)
}

pub(crate) fn cached<K: PartialEq, V>(
    drawn: &mut crate::model::Drawn<K, V>,
    key: K,
    draw: impl FnOnce() -> V,
) -> &V {
    let fresh = match drawn.take() {
        Some((at, rows)) if at == key => (at, rows),
        _ => (key, draw()),
    };
    &drawn.insert(fresh).1
}

/// Invariant: this scroll holds a view at its top while its rows grow, and a scroll down from
/// there leaves it less the rows, counted down from the top until the next draw.
pub(crate) const TOP: usize = usize::MAX;

/// The rows a view held at its top was scrolled down, or `None` for a count from the bottom.
pub(crate) fn down_from_top(from_bottom: usize) -> Option<usize> {
    let down = TOP.saturating_sub(from_bottom);
    (down < TOP / 2).then_some(down)
}

pub(crate) fn windowed(
    all: &[Line<'static>],
    from_bottom: &mut usize,
    visible: usize,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let total = all.len();
    let highest = total.saturating_sub(visible);
    // Scrolling past the top would bank rows a later scroll down must first spend.
    match down_from_top(*from_bottom) {
        Some(0) => {}
        Some(down) => *from_bottom = highest.saturating_sub(down),
        None => *from_bottom = (*from_bottom).min(highest),
    }
    let top = highest.saturating_sub((*from_bottom).min(highest));
    let rows = all.get(top..total.min(top.saturating_add(visible)));
    let extent = (total > visible).then_some((total, top));
    (
        rows.map(<[Line<'static>]>::to_vec).unwrap_or_default(),
        extent,
    )
}

const CODE_ROWS: usize = 24;
const STREAM_ROWS: usize = 40;
const RESULT_ROWS: usize = 20;
const TRACE_ROWS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Renderer {
    Markdown,
    Json,
    Code(&'static str),
    Text,
}

const RESULT_RENDERERS: [(&str, Renderer); RESULT_MIMES.len()] = [
    (RESULT_MIMES[0], Renderer::Markdown),
    (JSON_MIME, Renderer::Json),
    (RESULT_MIMES[2], Renderer::Code("python")),
];

fn renderer_for(mime: Option<&str>) -> Renderer {
    RESULT_RENDERERS
        .iter()
        .find(|(known, _)| Some(*known) == mime)
        .map_or(Renderer::Text, |(_, renderer)| *renderer)
}

fn cap_row(what: &str, cap: &str, next: &str, theme: &Theme) -> Line<'static> {
    Line::styled(
        format!("  […] {what} · {cap} · {next}"),
        theme.muted_style(),
    )
}

fn cap_notice(noun: &str, cap: usize, tail: bool, total: usize, theme: &Theme) -> Line<'static> {
    let what = format!(
        "{noun} {} {cap} of {total} rows",
        if tail { "last" } else { "first" }
    );
    cap_row(
        &what,
        &format!("notebook cap {cap}"),
        "v shows every row",
        theme,
    )
}

fn cap_first(total: usize, cap: usize, tail: bool, full: bool) -> Option<usize> {
    (total > cap && !full).then(|| if tail { total.saturating_sub(cap) } else { 0 })
}

/// Invariant: the notice sits where the rows it counts were dropped, so a tail cut leads with it.
fn with_notice(
    mut kept: Vec<Line<'static>>,
    notice: Line<'static>,
    tail: bool,
) -> Vec<Line<'static>> {
    if tail {
        kept.insert(0, notice);
    } else {
        kept.push(notice);
    }
    kept
}

fn cap_rows(
    mut rows: Vec<Line<'static>>,
    (cap, tail, full): (usize, bool, bool),
    noun: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let total = rows.len();
    let Some(first) = cap_first(total, cap, tail, full) else {
        return rows;
    };
    let notice = cap_notice(noun, cap, tail, total, theme);
    let kept = if tail {
        rows.split_off(first)
    } else {
        rows.truncate(cap);
        rows
    };
    with_notice(kept, notice, tail)
}
fn trace_rows(
    text: &str,
    code: &str,
    width: usize,
    lead: Span<'static>,
    (cap, tail, full): (usize, bool, bool),
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(lead.width());
    let rows = yi_tui::pycell::traceback_rows(text);
    let total = rows.len();
    // Invariant: the marker names the exception even when the block is tail-capped, so it lands
    // on the last exception raised and never on a row the cap cuts away.
    let mark = rows
        .iter()
        .rposition(|(kind, _)| *kind == yi_tui::pycell::TraceRow::Raised)
        .or_else(|| rows.iter().rposition(|(_, line)| !line.trim().is_empty()))
        .unwrap_or(0);
    let cut = cap_first(total, cap, tail, full);
    let kept = match cut {
        Some(first) if tail => first..total,
        Some(_) => 0..cap.min(total),
        None => 0..total,
    };
    let mut lang = yi_tui::highlight::lang_for(yi_tui::pycell::chip(code));
    let mut out: Vec<Line<'static>> = Vec::new();
    for (at, (kind, line)) in rows.iter().enumerate().skip(kept.start).take(kept.len()) {
        let mut spans = vec![if at == mark {
            lead.clone()
        } else {
            Span::raw(indent.clone())
        }];
        spans.extend(yi_tui::pycell::row_spans(*kind, line, lang.as_mut(), theme));
        out.extend(yi_tui::wrap::wrap_line(&Line::from(spans), width, &indent));
    }
    match cut {
        Some(_) => with_notice(out, cap_notice("traceback", cap, tail, total, theme), tail),
        None => out,
    }
}

fn highlight_line(
    line: &str,
    lang: Option<&mut yi_tui::highlight::Lang>,
    theme: &Theme,
    base: Style,
) -> Vec<Span<'static>> {
    match lang {
        Some(lang) => yi_tui::highlight::spans(line, lang, theme, base),
        None => vec![Span::styled(line.to_owned(), base)],
    }
}

fn capped(
    text: &str,
    lead: Span<'static>,
    style: Style,
    language: Option<&str>,
    (width, cap, tail, full): (usize, usize, bool, bool),
    noun: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(lead.width());
    let mut lang = language.and_then(yi_tui::highlight::lang_for);
    let rows: Vec<Line<'static>> = text
        .lines()
        .flat_map(|line| {
            let clean = yi_tui::transcript::show_controls(line);
            let mut spans = vec![lead.clone()];
            spans.extend(highlight_line(&clean, lang.as_mut(), theme, style));
            yi_tui::wrap::wrap_line(&Line::from(spans), width, &indent)
        })
        .collect();
    cap_rows(rows, (cap, tail, full), noun, theme)
}

fn markdown_rows(
    source: &str,
    lead: &Span<'static>,
    (width, cap, tail, full): (usize, usize, bool, bool),
    noun: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let cleaned = source
        .lines()
        .map(yi_tui::transcript::show_controls)
        .collect::<Vec<String>>()
        .join("\n");
    let rendered = yi_tui::markdown::render(&cleaned, width.saturating_sub(lead.width()), theme);
    let rows = rendered
        .into_iter()
        .map(|mut row| {
            row.spans.insert(0, lead.clone());
            row
        })
        .collect();
    cap_rows(rows, (cap, tail, full), noun, theme)
}

fn result_rows(
    result: &str,
    mime: Option<&str>,
    lead: Span<'static>,
    style: Style,
    shape: (usize, usize, bool, bool),
    theme: &Theme,
) -> Vec<Line<'static>> {
    if result.trim().is_empty() {
        return Vec::new();
    }
    match renderer_for(mime) {
        Renderer::Markdown => markdown_rows(result, &lead, shape, "result", theme),
        Renderer::Json => match serde_json::from_str::<serde_json::Value>(result) {
            Ok(value) => {
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| result.to_owned());
                capped(&pretty, lead, style, Some("json"), shape, "result", theme)
            }
            Err(_) => {
                let mut rows = capped(result, lead, style, None, shape, "result", theme);
                let why = if result
                    .rsplit('\n')
                    .next()
                    .is_some_and(|row| row.starts_with("[…"))
                {
                    "application/json result is past the cell's byte cap · the transcript holds all of it"
                } else {
                    "application/json result did not parse as JSON · shown as text"
                };
                rows.push(Line::styled(format!("  […] {why}"), theme.muted_style()));
                rows
            }
        },
        Renderer::Code(language) => {
            capped(result, lead, style, Some(language), shape, "result", theme)
        }
        Renderer::Text => capped(result, lead, style, None, shape, "result", theme),
    }
}

/// Incident: a `│` gutter read as part of the `cargo tree` box-drawing a cell parsed, so rows
/// are numbered; a wrapped row hangs past its own indent so the Python still reads.
fn code_rows(code: &str, width: usize, full: bool, theme: &Theme) -> Vec<Line<'static>> {
    let lines: Vec<String> = code
        .lines()
        .map(|line| yi_tui::pycell::redact(&yi_tui::transcript::show_controls(line)))
        .collect();
    let gutter = lines.len().to_string().len().max(2);
    let shown = if full {
        lines.len()
    } else {
        lines.len().min(CODE_ROWS)
    };
    let mut lang = yi_tui::highlight::lang_for(yi_tui::pycell::chip(code));
    let base = Style::default().fg(theme.text);
    let mut out = Vec::new();
    for (index, line) in lines.iter().take(shown).enumerate() {
        let spans = highlight_line(line, lang.as_mut(), theme, base);
        let depth = line.chars().take_while(|c| c.is_whitespace()).count();
        let hang = " ".repeat(depth.min(width / 2).saturating_add(4));
        let body = width.saturating_sub(gutter.saturating_add(1));
        let wrapped = yi_tui::wrap::wrap_line(&Line::from(spans), body, &hang);
        for (row, mut wrapped) in wrapped.into_iter().enumerate() {
            let mark = if row == 0 {
                format!("{:>gutter$} ", index.saturating_add(1))
            } else {
                format!("{:>gutter$} ", "↪")
            };
            wrapped
                .spans
                .insert(0, Span::styled(mark, theme.dim_style()));
            out.push(wrapped);
        }
    }
    if shown < lines.len() {
        let what = format!("code first {shown} of {} lines", lines.len());
        out.push(cap_row(
            &what,
            &format!("notebook cap {CODE_ROWS}"),
            "v shows every row",
            theme,
        ));
    }
    out
}

pub fn notebook_lines(
    cells: &[crate::model::NbCell],
    width: usize,
    full: bool,
    theme: &Theme,
    close: &str,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let Some(first) = cells.first() else {
        out.push(Line::styled(
            "no kernel cells yet — they appear as the agent computes",
            theme.dim_style(),
        ));
        out.push(Line::styled(
            format!("{close} closes · an idle notebook closes itself"),
            theme.dim_style(),
        ));
        return out;
    };
    if first.number > 1 {
        let dropped = first.number.saturating_sub(1);
        let noun = if dropped == 1 { "cell" } else { "cells" };
        let what = format!("{dropped} older {noun} not shown");
        let cap = format!("notebook keeps {}", crate::model::KEPT_CELLS);
        out.push(cap_row(
            &what,
            &cap,
            "the transcript holds every call",
            theme,
        ));
    }
    let text = Style::default().fg(theme.text);
    let error = Style::default().fg(theme.error);
    for cell in cells {
        let (marker, style) = if cell.running {
            ("◐", Style::default().fg(theme.warning))
        } else if cell.error.is_some() {
            ("●", error)
        } else {
            ("●", Style::default().fg(theme.success))
        };
        let label = format!("{marker} [{}]", cell.number);
        let took = match cell.duration_ms {
            _ if cell.running => "running".to_owned(),
            Some(ms) => yi_tui::cell::elapsed_label(ms),
            None => String::new(),
        };
        let pad = width.saturating_sub(label.chars().count() + took.chars().count());
        out.push(Line::from(vec![
            Span::styled(label, style.add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(pad)),
            Span::styled(took, theme.dim_style()),
        ]));
        out.extend(code_rows(&cell.code, width, full, theme));
        let shape = |cap, tail| (width, cap, tail, full);
        let pad = Span::raw("  ");
        let warn = Style::default().fg(theme.warning);
        out.extend(capped(
            &cell.stdout,
            pad.clone(),
            text,
            None,
            shape(STREAM_ROWS, false),
            "stdout",
            theme,
        ));
        out.extend(capped(
            &cell.stderr,
            pad,
            warn,
            None,
            shape(STREAM_ROWS, false),
            "stderr",
            theme,
        ));
        let teal = Style::default().fg(theme.teal);
        if let Some(result) = &cell.result {
            out.extend(result_rows(
                result,
                cell.result_mime.as_deref(),
                Span::styled("↳ ", teal),
                teal.add_modifier(Modifier::BOLD),
                shape(RESULT_ROWS, false),
                theme,
            ));
        }
        if let Some(trace) = &cell.error {
            let lead = Span::styled("✕ ", error);
            out.extend(trace_rows(
                trace,
                &cell.code,
                width,
                lead,
                (TRACE_ROWS, true, full),
                theme,
            ));
        }
        for size in &cell.dropped_image_bytes {
            let what = format!("image {size} B base64 dropped");
            let cap = format!("notebook cap {} B", crate::model::IMAGE_BYTES_MAX);
            out.push(cap_row(&what, &cap, "the transcript holds it", theme));
        }
        if cell.images_past_limit > 0 {
            let noun = if cell.images_past_limit == 1 {
                "image"
            } else {
                "images"
            };
            let what = format!("{} older {noun} not shown", cell.images_past_limit);
            let cap = format!("notebook keeps {} per cell", crate::model::IMAGES_PER_CELL);
            out.push(cap_row(&what, &cap, "the transcript holds them", theme));
        }
        for bytes in &cell.image_bytes {
            let kb = bytes.saturating_mul(3) / 4 / 1024;
            out.push(Line::styled(format!("  ▲ image · {kb} KB png"), teal));
        }
        out.push(Line::default());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Renderer, renderer_for};
    use yi_types::kernel::RESULT_MIMES;

    /// Dies with the kernel able to name a bundle the pane draws as flat text: the mime would
    /// reach the cell and the pane would show the repr the richer bundle replaced.
    #[test]
    fn every_mime_the_kernel_ranks_by_has_a_renderer() {
        for mime in RESULT_MIMES {
            assert!(
                !matches!(renderer_for(Some(mime)), Renderer::Text),
                "{mime} draws as flat text"
            );
        }
    }
}
