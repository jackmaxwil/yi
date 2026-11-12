use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};
use yi_types::kernel::{JSON_MIME, MARKDOWN_MIME, PYTHON_MIME};

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

/// What a click in the review's pinned header picks: a scope chip or a file row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPick {
    Scope(crate::model::ReviewScope),
    File(usize),
}

/// A review pane's state beside what it shows: the file the body should scroll to on the next draw,
/// and where the last draw put each pick.
#[derive(Default)]
pub struct ReviewUi {
    pub(crate) jump: Option<usize>,
    pub(crate) hits: Vec<(ratatui::layout::Rect, ReviewPick)>,
}

/// A review as drawn: a header pinned above the pane's scroll, and the diff that scrolls under it.
pub struct Review {
    pub(crate) title: String,
    pub(crate) head: Vec<Line<'static>>,
    pub(crate) body: Vec<Line<'static>>,
    /// Each scope chip's column, width and scope, from the pane's left edge.
    pub(crate) scopes: Vec<(u16, u16, crate::model::ReviewScope)>,
    /// Each drawn file row's place in `head`, and which file it is.
    pub(crate) files: Vec<(usize, usize)>,
    /// The row of `body` each file's header sits on.
    pub(crate) heads: Vec<usize>,
}

/// Rows the file list shows at most; the rest are named in a row of its own.
const FILE_ROWS: usize = 8;
const BAR: usize = 5;

fn scope_count(
    diff: Option<&crate::model::SessionDiff>,
    scope: crate::model::ReviewScope,
) -> usize {
    use crate::model::ReviewScope;
    let Some(diff) = diff else { return 0 };
    match scope {
        ReviewScope::Branch => diff.branch.as_ref().map_or(0, |branch| branch.files.len()),
        ReviewScope::Turn => diff
            .files
            .iter()
            .filter(|(_, file)| file.turn == diff.turn)
            .count(),
        ReviewScope::Session => diff.files.len(),
    }
}

/// A for a file the patch creates, D for one it deletes, U for one git does not track yet.
fn status_of(patch: &str, tracked: bool) -> char {
    let made = patch.contains("\nnew file mode") || patch.contains("--- /dev/null");
    let gone = patch.contains("\ndeleted file mode") || patch.contains("+++ /dev/null");
    match (tracked, made, gone) {
        (false, _, _) => 'U',
        (_, true, _) => 'A',
        (_, _, true) => 'D',
        _ => 'M',
    }
}

fn status_style(status: char, theme: &Theme) -> Style {
    let color = match status {
        'A' | 'U' => theme.success,
        'D' => theme.error,
        _ => theme.warning,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// Five cells of the change's shape: green for what it adds, red for what it removes.
fn change_bar(added: u64, removed: u64, theme: &Theme) -> Vec<Span<'static>> {
    let total = added.saturating_add(removed);
    if total == 0 {
        return Vec::new();
    }
    let cells = u64::try_from(BAR).unwrap_or(5);
    let mut green = added.saturating_mul(cells).saturating_add(total / 2) / total;
    if added > 0 && green == 0 {
        green = 1;
    }
    if removed > 0 && green >= cells {
        green = cells.saturating_sub(1);
    }
    let (green, red) = (
        usize::try_from(green).unwrap_or(0),
        usize::try_from(
            cells
                .saturating_sub(green)
                .min(if removed > 0 { cells } else { 0 }),
        )
        .unwrap_or(0),
    );
    vec![
        Span::styled("█".repeat(green), Style::default().fg(theme.success)),
        Span::styled("█".repeat(red), Style::default().fg(theme.error)),
    ]
}

/// The review of `scope`: its header with `room` rows for the file list, and its diff body.
pub fn review_parts(
    diff: Option<&crate::model::SessionDiff>,
    scope: crate::model::ReviewScope,
    note: &str,
    (width, room): (usize, usize),
    theme: &Theme,
) -> Review {
    use crate::model::ReviewScope;
    let unbased = scope == ReviewScope::Branch && diff.is_none_or(|diff| diff.branch.is_none());
    let scope = if unbased { ReviewScope::Session } else { scope };
    let patches: Vec<(String, String)> =
        diff.map_or_else(Vec::new, |diff| review_files(diff, scope));
    let (counts, untracked): (Vec<(u64, u64, bool)>, String) = match (scope, diff) {
        (ReviewScope::Branch, Some(diff)) => diff.branch.as_ref().map_or_else(
            || (Vec::new(), String::new()),
            |branch| {
                let loose = if branch.untracked == 0 {
                    String::new()
                } else {
                    format!(" · {} untracked", branch.untracked)
                };
                (
                    branch
                        .files
                        .iter()
                        .map(|(_, add, rem)| (*add, *rem, true))
                        .collect(),
                    format!("branch vs base {}{loose}", branch.base),
                )
            },
        ),
        (_, Some(diff)) => {
            let chosen: Vec<&crate::model::FileDiff> = diff
                .files
                .iter()
                .filter(|(_, file)| scope == ReviewScope::Session || file.turn == diff.turn)
                .map(|(_, file)| file)
                .collect();
            let loose = match chosen.iter().filter(|file| !file.tracked).count() {
                0 => String::new(),
                loose => format!(" · {loose} untracked"),
            };
            (
                chosen
                    .iter()
                    .map(|file| (file.added, file.removed, file.tracked))
                    .collect(),
                loose,
            )
        }
        (_, None) => (Vec::new(), String::new()),
    };
    let serving = |path: &str| {
        diff.and_then(|diff| diff.files.iter().find(|(known, _)| same_file(known, path)))
            .and_then(|(_, file)| file.serving.clone())
    };
    let (added, removed) = counts.iter().fold((0_u64, 0_u64), |(a, r), (add, rem, _)| {
        (a.saturating_add(*add), r.saturating_add(*rem))
    });
    let noun = if patches.len() == 1 { "file" } else { "files" };
    let title = format!(
        "Review · {} · {} {noun} +{added} −{removed}{note}",
        scope.label(),
        patches.len()
    );
    let base = match (unbased, scope) {
        (true, _) => format!("no branch diff from git, so this session's edits{untracked}"),
        (false, ReviewScope::Branch) => untracked,
        (false, ReviewScope::Turn) => format!("the last turn's edits{untracked}"),
        (false, ReviewScope::Session) => format!("this session's edits{untracked}"),
    };
    // The chips: each scope and how many files it holds, the live one lit.
    let mut chips: Vec<Span<'static>> = Vec::new();
    let mut scopes = Vec::new();
    let mut at = 0_u16;
    for chip in [ReviewScope::Branch, ReviewScope::Turn, ReviewScope::Session] {
        let label = format!(" {} {} ", chip.label(), scope_count(diff, chip));
        let span = u16::try_from(label.chars().count()).unwrap_or(0);
        let style = if chip == scope {
            Style::default()
                .fg(theme.text)
                .bg(theme.selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            theme.muted_style()
        };
        chips.push(Span::styled(label, style));
        chips.push(Span::raw(" "));
        scopes.push((at, span, chip));
        at = at.saturating_add(span).saturating_add(1);
    }
    let sums = format!("+{added} −{removed} ");
    let pad = width
        .saturating_sub(usize::from(at))
        .saturating_sub(sums.chars().count());
    chips.push(Span::raw(" ".repeat(pad)));
    chips.push(Span::styled(
        format!("+{added} "),
        Style::default().fg(theme.success),
    ));
    chips.push(Span::styled(
        format!("−{removed} "),
        Style::default().fg(theme.error),
    ));
    let mut head: Vec<Line<'static>> =
        vec![Line::from(chips), Line::styled(base, theme.dim_style())];
    if let Some(title) = diff.and_then(|diff| diff.land_armed.as_deref()) {
        head.push(Line::styled(
            format!("l again to /land {title} · any other key cancels"),
            Style::default().fg(theme.warning),
        ));
    }
    let selected = diff.map_or(0, |diff| diff.selected.min(patches.len().saturating_sub(1)));
    let shown = room.clamp(1, FILE_ROWS).min(patches.len().max(1));
    let first = selected
        .saturating_sub(shown / 2)
        .min(patches.len().saturating_sub(shown));
    let mut files = Vec::new();
    for (index, (path, patch)) in patches.iter().enumerate().skip(first).take(shown) {
        let (add, rem, tracked) = counts.get(index).copied().unwrap_or((0, 0, true));
        let status = status_of(patch, tracked);
        let mark = if index == selected { "▸" } else { " " };
        let band = if index == selected {
            Style::default().bg(theme.selection_bg())
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(format!("{mark} "), band.fg(theme.text)),
            Span::styled(
                format!("{status} "),
                band.patch(status_style(status, theme)),
            ),
            Span::styled(path.clone(), band.fg(theme.text)),
            Span::styled(format!("  +{add} −{rem} "), band.patch(theme.dim_style())),
        ];
        spans.extend(change_bar(add, rem, theme));
        if let Some(label) = serving(path) {
            spans.push(Span::styled(format!("  · {label}"), theme.muted_style()));
        }
        files.push((head.len(), index));
        head.push(Line::from(spans));
    }
    if patches.len() > shown {
        head.push(Line::styled(
            format!(
                "  […] {shown} of {} files listed · ↑/↓ moves the list · the diff below scrolls",
                patches.len()
            ),
            theme.muted_style(),
        ));
    }
    let read_only = diff.map_or(0, |diff| {
        diff.reads
            .keys()
            .filter(|path| !patches.iter().any(|(changed, _)| same_file(path, changed)))
            .count()
    });
    if read_only > 0 {
        let noun = if read_only == 1 { "file" } else { "files" };
        head.push(Line::styled(
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
        head.push(cap_row(
            &format!("{dropped} older {noun} not shown"),
            &cap,
            "git diff lists them",
            theme,
        ));
    }
    head.push(Line::styled(
        "s scope · ↑/↓ file · w why each hunk · l land under the session's title",
        theme.dim_style(),
    ));
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut heads = Vec::new();
    if patches.is_empty() {
        let others: Vec<String> = [ReviewScope::Branch, ReviewScope::Turn, ReviewScope::Session]
            .iter()
            .map(|other| format!("{} {}", other.label(), scope_count(diff, *other)))
            .collect();
        body.push(Line::default());
        body.push(Line::styled(
            "nothing changed in this scope",
            theme.dim_style(),
        ));
        body.push(Line::styled(
            format!("{} · click a scope above", others.join(" · ")),
            theme.dim_style(),
        ));
    }
    for (index, (path, patch)) in patches.iter().enumerate() {
        let (add, rem, tracked) = counts.get(index).copied().unwrap_or((0, 0, true));
        let status = status_of(patch, tracked);
        heads.push(body.len());
        body.push(
            Line::from(vec![
                Span::styled(format!("{status} "), status_style(status, theme)),
                Span::styled(
                    path.clone(),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  +{add} −{rem}"), theme.dim_style()),
            ])
            .style(Style::default().bg(theme.selection_bg())),
        );
        if let Some(chain) = diff.and_then(|diff| diff.why.get(path)) {
            body.extend(
                chain
                    .iter()
                    .map(|row| Line::styled(row.clone(), theme.muted_style())),
            );
        }
        body.extend(diffview::render(patch, width, theme, DiffBudget::FULL));
        body.push(Line::default());
    }
    Review {
        title,
        head,
        body,
        scopes,
        files,
        heads,
    }
}

/// Where each chip and file row of a drawn review sits in a pane whose text starts at `inner`.
pub(crate) fn review_hits(
    review: &Review,
    inner: ratatui::layout::Rect,
) -> Vec<(ratatui::layout::Rect, ReviewPick)> {
    use ratatui::layout::Rect;
    let chips = review.scopes.iter().map(|(x, w, scope)| {
        let at = inner.x.saturating_add(*x);
        (Rect::new(at, inner.y, *w, 1), ReviewPick::Scope(*scope))
    });
    let rows = review.files.iter().map(|(row, index)| {
        let y = inner.y.saturating_add(u16::try_from(*row).unwrap_or(0));
        (
            Rect::new(inner.x, y, inner.width, 1),
            ReviewPick::File(*index),
        )
    });
    chips.chain(rows).collect()
}

/// Each chip and file row the last draw laid out is a control, for this frame's panels only.
pub(crate) fn offer_review(app: &mut crate::app::App) {
    let mut found = Vec::new();
    for id in crate::hit::drawn(app) {
        let Some(pane) = app.state.panes.get(&id) else {
            continue;
        };
        if let crate::model::PaneContent::SessionDiff { ui, .. } = &pane.content {
            found.extend(
                ui.hits
                    .iter()
                    .map(|(rect, pick)| (*rect, crate::hit::Hit::Review(id, *pick))),
            );
        }
    }
    crate::hit::offer(app, found);
}

/// The review as one column of text: header rows, then the diff.
pub fn review_view(
    diff: Option<&crate::model::SessionDiff>,
    scope: crate::model::ReviewScope,
    note: &str,
    width: usize,
    theme: &Theme,
) -> (String, Vec<Line<'static>>) {
    let review = review_parts(diff, scope, note, (width, FILE_ROWS), theme);
    let mut lines = review.head;
    lines.extend(review.body);
    (review.title, lines)
}

pub(crate) fn cached<K: PartialEq, V>(
    drawn: &mut crate::model::Drawn<K, V>,
    key: K,
    palette: u64,
    draw: impl FnOnce() -> V,
) -> &V {
    let fresh = match drawn.take() {
        Some((at, painted, rows)) if at == key && painted == palette => (at, painted, rows),
        _ => (key, palette, draw()),
    };
    &drawn.insert(fresh).2
}

/// Invariant: this scroll holds a view at its top while its rows grow, and a scroll down from
/// there leaves it less the rows, counted down from the top until the next draw.
pub(crate) const TOP: usize = usize::MAX;

/// The rows a view held at its top was scrolled down, or `None` for a count from the bottom.
pub(crate) fn down_from_top(from_bottom: usize) -> Option<usize> {
    let down = TOP.saturating_sub(from_bottom);
    (down < TOP / 2).then_some(down)
}

/// The first row of a `room`-tall window that keeps `cursor` in view: its last row once past it.
pub(crate) fn first_row(cursor: usize, room: usize) -> usize {
    cursor.saturating_add(1).saturating_sub(room)
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

const RESULT_RENDERERS: [(&str, Renderer); 3] = [
    (MARKDOWN_MIME, Renderer::Markdown),
    (JSON_MIME, Renderer::Json),
    (PYTHON_MIME, Renderer::Code("python")),
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

/// Incident: the notebook's byte-cap notice rode inside a markdown table or fence and drew as a
/// cell or as code; only a result the cell says was byte-capped has its notice peeled off.
fn split_notice(result: &str, byte_capped: bool) -> (&str, Option<&str>) {
    match result.rsplit_once('\n') {
        Some((body, row)) if byte_capped => (body, Some(row)),
        _ => (result, None),
    }
}

fn result_rows(
    result: &str,
    mime: Option<&str>,
    byte_capped: bool,
    lead: Span<'static>,
    style: Style,
    shape: (usize, usize, bool, bool),
    theme: &Theme,
) -> Vec<Line<'static>> {
    if result.trim().is_empty() {
        return Vec::new();
    }
    match renderer_for(mime) {
        Renderer::Markdown => {
            let (body, notice) = split_notice(result, byte_capped);
            let mut rows = markdown_rows(body, &lead, shape, "result", theme);
            rows.extend(notice.map(|row| Line::styled(format!("  {row}"), theme.muted_style())));
            rows
        }
        Renderer::Json => match serde_json::from_str::<serde_json::Value>(result) {
            Ok(value) => {
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| result.to_owned());
                capped(&pretty, lead, style, Some("json"), shape, "result", theme)
            }
            Err(_) => {
                let mut rows = capped(result, lead, style, None, shape, "result", theme);
                let why = if byte_capped {
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
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let Some(first) = cells.first() else {
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
                cell.result_capped,
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
