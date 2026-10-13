use ratatui::style::Style;
use ratatui::text::{Line, Span};
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};

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
