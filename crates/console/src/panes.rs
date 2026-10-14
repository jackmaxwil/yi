use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};
use yi_types::status_mark::StatusMark;

fn span_of(ms: u64) -> String {
    match ms / 1000 {
        secs @ 0..60 => format!("{secs}s"),
        secs @ 60..3600 => format!("{}m", secs / 60),
        secs => format!("{}h{}m", secs / 3600, secs / 60 % 60),
    }
}

pub fn tape_view(
    tape: Option<&yi_types::tape::Tape>,
    cursor: usize,
    width: usize,
    theme: &Theme,
) -> (String, Vec<Line<'static>>) {
    use yi_types::tape::MarkKind;
    let Some(tape) = tape.filter(|tape| tape.end > tape.start) else {
        return (
            "Tape".to_owned(),
            vec![Line::styled(
                "no turns on the ledger yet",
                theme.dim_style(),
            )],
        );
    };
    let total = tape.end - tape.start;
    let cols = width.saturating_sub(8).max(10);
    let col_of = |at: u64| {
        let offset = u128::from(at.saturating_sub(tape.start));
        usize::try_from(offset * cols as u128 / u128::from(total))
            .unwrap_or(cols)
            .min(cols - 1)
    };
    let covered = |spans: &[[u64; 2]]| -> (String, u64) {
        let mut fill = vec![0_u64; cols];
        let mut sum = 0_u64;
        for [from, to] in spans {
            sum = sum.saturating_add(to.saturating_sub(*from));
            for (col, cell) in fill.iter_mut().enumerate() {
                let lo = tape.start + total * col as u64 / cols as u64;
                let hi = tape.start + total * (col as u64 + 1) / cols as u64;
                *cell += (*to).min(hi).saturating_sub((*from).max(lo));
            }
        }
        let bar = fill
            .iter()
            .enumerate()
            .map(|(col, ms)| {
                let span = (total * (col as u64 + 1) / cols as u64)
                    .saturating_sub(total * col as u64 / cols as u64)
                    .max(1);
                let level = usize::try_from(ms * 8 / span).unwrap_or(8).min(8);
                [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][level]
            })
            .collect();
        (bar, sum * 100 / total)
    };
    let (model, model_share) = covered(&tape.model);
    let (tools, tools_share) = covered(&tape.tools);
    let mut you = vec![' '; cols];
    let mut marks = vec![' '; cols];
    for mark in &tape.marks {
        let col = col_of(mark.at);
        match mark.kind {
            MarkKind::User => you[col] = '┃',
            MarkKind::Checkpoint => marks[col] = '◆',
            MarkKind::Failed => marks[col] = StatusMark::Failed.symbol(),
            MarkKind::Compaction => marks[col] = '⌇',
            MarkKind::Other(_) => marks[col] = '·',
        }
    }
    let track = |name: &str, body: String| {
        Line::from(vec![
            Span::styled(format!("{name:<6}▕"), theme.dim_style()),
            Span::styled(body, Style::default().fg(theme.text)),
            Span::styled("▏", theme.dim_style()),
        ])
    };
    let mut lines = vec![
        track("model", model),
        track("tools", tools),
        track("you", you.into_iter().collect()),
        track("marks", marks.into_iter().collect()),
    ];
    if let Some(mark) = tape.marks.get(cursor) {
        let pointer = format!("{}▲", " ".repeat(col_of(mark.at).saturating_add(7)));
        lines.push(Line::styled(pointer, theme.accent_style()));
        let then = span_of(mark.at.saturating_sub(tape.start));
        lines.push(Line::styled(
            format!("+{then} · {}", mark.label),
            Style::default().fg(theme.text),
        ));
    }
    lines.push(Line::styled(
        format!(
            "◆ checkpoint  {} failed  ⌇ compaction  ┃ you · ←/→ marks · enter rewinds to your turn · u u also restores its files",
            StatusMark::Failed.glyph()
        ),
        theme.dim_style(),
    ));
    let title = format!(
        "Tape · {} · model {model_share}% · tools {tools_share}%",
        span_of(total)
    );
    (title, lines)
}

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
            theme.accent_style(),
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

const CODE_ROWS: usize = 24;
const STREAM_ROWS: usize = 40;
const RESULT_ROWS: usize = 20;
const TRACE_ROWS: usize = 12;

fn cap_row(what: &str, cap: &str, next: &str, theme: &Theme) -> Line<'static> {
    Line::styled(
        format!("  […] {what} · {cap} · {next}"),
        theme.muted_style(),
    )
}

fn capped(
    text: &str,
    lead: Span<'static>,
    style: Style,
    (width, cap, tail, full): (usize, usize, bool, bool),
    noun: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(lead.width());
    let mut rows: Vec<Line<'static>> = text
        .lines()
        .flat_map(|line| {
            let line = Line::from(vec![
                lead.clone(),
                Span::styled(yi_tui::transcript::show_controls(line), style),
            ]);
            yi_tui::wrap::wrap_line(&line, width, &indent)
        })
        .collect();
    let total = rows.len();
    if full || total <= cap {
        return rows;
    }
    let what = format!(
        "{noun} {} {cap} of {total} rows",
        if tail { "last" } else { "first" }
    );
    let row = cap_row(
        &what,
        &format!("notebook cap {cap}"),
        "v shows every row",
        theme,
    );
    if tail {
        let mut kept = rows.split_off(total.saturating_sub(cap));
        kept.insert(0, row);
        kept
    } else {
        rows.truncate(cap);
        rows.push(row);
        rows
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
    let mut lang = yi_tui::highlight::lang_for("python");
    let base = Style::default().fg(theme.text);
    let mut out = Vec::new();
    for (index, line) in lines.iter().take(shown).enumerate() {
        let spans = match lang.as_mut() {
            Some(lang) => yi_tui::highlight::spans(line.as_str(), lang, theme, base),
            None => vec![Span::styled(line.clone(), base)],
        };
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
            shape(STREAM_ROWS, false),
            "stdout",
            theme,
        ));
        out.extend(capped(
            &cell.stderr,
            pad,
            warn,
            shape(STREAM_ROWS, false),
            "stderr",
            theme,
        ));
        if let Some(result) = &cell.result {
            let lead = Span::styled("↳ ", theme.accent_style());
            let bold = theme.accent_style().add_modifier(Modifier::BOLD);
            out.extend(capped(
                result,
                lead,
                bold,
                shape(RESULT_ROWS, false),
                "result",
                theme,
            ));
        }
        if let Some(trace) = &cell.error {
            let lead = Span::styled("✕ ", error);
            out.extend(capped(
                trace,
                lead,
                error,
                shape(TRACE_ROWS, true),
                "traceback",
                theme,
            ));
        }
        for image in &cell.images {
            let kb = image.len().saturating_mul(3) / 4 / 1024;
            out.push(Line::styled(
                format!("  ▲ image · {kb} KB png"),
                theme.accent_style(),
            ));
        }
        out.push(Line::default());
    }
    out
}
