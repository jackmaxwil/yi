use ratatui::style::Style;
use ratatui::text::{Line, Span};
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};

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
            MarkKind::Failed => marks[col] = '✗',
            MarkKind::Compaction => marks[col] = '⌇',
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
        "◆ checkpoint  ✗ failed  ⌇ compaction  ┃ you · ←/→ marks · enter rewinds to your turn · u u also restores its files",
        theme.dim_style(),
    ));
    let title = format!(
        "Tape · {} · model {model_share}% · tools {tools_share}%",
        span_of(total)
    );
    (title, lines)
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
        diff.and_then(|diff| diff.files.iter().find(|(known, _)| known == path))
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
            .filter(|path| {
                !files
                    .iter()
                    .any(|(changed, _, _)| std::path::Path::new(path).ends_with(changed))
            })
            .count()
    });
    if read_only > 0 {
        lines.push(Line::styled(
            format!("○ {read_only} files read only"),
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
