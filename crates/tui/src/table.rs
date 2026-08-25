// codex markdown table pipeline, port adapted (A.10 pass-3): styled span
// cells, spillover filtering, column-count normalization, content-aware
// width allocation with priority shrinking, aligned grid rendering with
// in-cell wrapping, and the key/value record fallback when a grid no
// longer scans. Hyperlink remapping and HTML-spillover heuristics dropped.

use pulldown_cmark::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::colors::Theme;
use crate::wrap::wrap_line;

const TABLE_COLUMN_GAP: usize = 2;
const TABLE_CELL_PADDING: usize = 1;
const TABLE_HEADER_SEPARATOR_CHAR: char = '━';
const TABLE_BODY_SEPARATOR_CHAR: char = '─';
const MIN_COLUMN_WIDTH: usize = 3;

const FIELD_LEADING_PADDING: usize = 1;
const FIELD_GAP: usize = 2;
const MIN_VALUE_WIDTH: usize = 3;
const MIN_ALIGNED_COMPACT_VALUE_WIDTH: usize = 12;
const MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH: usize = 24;
const MIN_SCANNABLE_NARRATIVE_WIDTH: usize = 12;
const MIN_SCANNABLE_TOKEN_HEAVY_WIDTH: usize = 12;
const CRAMPED_EXPANSIVE_CELL_LINES: usize = 4;
const CATASTROPHIC_NARRATIVE_CELL_LINES: usize = 7;
const STACKED_VALUE_INDENT: usize = 2;

#[derive(Debug, Clone, Default)]
pub struct TableCell {
    pub lines: Vec<Line<'static>>,
}

impl TableCell {
    fn ensure_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(Line::default());
        }
    }

    pub fn push_span(&mut self, span: Span<'static>) {
        self.ensure_line();
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(span);
        }
    }

    pub fn hard_break(&mut self) {
        self.lines.push(Line::default());
    }

    fn plain_text(&self) -> String {
        let mut buf = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                buf.push(' ');
            }
            for span in &line.spans {
                buf.push_str(span.content.as_ref());
            }
        }
        buf
    }

    fn display_width(&self) -> usize {
        self.lines.iter().map(line_width).max().unwrap_or(0)
    }
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|s| s.content.as_ref().width()).sum()
}

/// Token-heavy columns (paths, hashes) give up width before narrative prose;
/// compact values (counts, labels) resist wrapping and are preserved last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnKind {
    Narrative,
    TokenHeavy,
    Compact,
}

#[derive(Clone, Debug)]
struct ColumnMetrics {
    max_width: usize,
    header_token_width: usize,
    body_token_width: usize,
    kind: ColumnKind,
}

fn longest_token_width(text: &str) -> usize {
    text.split_whitespace()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

fn collect_metrics(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    columns: usize,
) -> Vec<ColumnMetrics> {
    let mut metrics = Vec::with_capacity(columns);
    for column in 0..columns {
        let header_plain = header
            .get(column)
            .map(TableCell::plain_text)
            .unwrap_or_default();
        let mut max_width = header
            .get(column)
            .map(TableCell::display_width)
            .unwrap_or(0);
        let mut body_token_width = 0_usize;
        let mut body_token_count = 0_usize;
        let mut long_body_token_count = 0_usize;
        let mut total_words = 0_usize;
        let mut total_cells = 0_usize;
        let mut total_cell_width = 0_usize;
        for row in rows {
            let Some(cell) = row.get(column) else {
                continue;
            };
            max_width = max_width.max(cell.display_width());
            let plain = cell.plain_text();
            let mut word_count = 0_usize;
            for token in plain.split_whitespace() {
                let token_width = token.width();
                body_token_width = body_token_width.max(token_width);
                long_body_token_count += usize::from(token_width >= 20);
                word_count += 1;
            }
            if word_count > 0 {
                body_token_count += word_count;
                total_words += word_count;
                total_cells += 1;
                total_cell_width += plain.width();
            }
        }
        let avg_words = if total_cells == 0 {
            header_plain.split_whitespace().count() as f64
        } else {
            total_words as f64 / total_cells as f64
        };
        let avg_width = if total_cells == 0 {
            header_plain.width() as f64
        } else {
            total_cell_width as f64 / total_cells as f64
        };
        let kind = if long_body_token_count > 0
            && long_body_token_count >= body_token_count.saturating_sub(long_body_token_count)
        {
            ColumnKind::TokenHeavy
        } else if avg_words >= 4.0 || avg_width >= 28.0 {
            ColumnKind::Narrative
        } else {
            ColumnKind::Compact
        };
        metrics.push(ColumnMetrics {
            max_width,
            header_token_width: longest_token_width(&header_plain),
            body_token_width,
            kind,
        });
    }
    metrics
}

/// Narrative/token-heavy columns keep a readable 16-cell soft floor; compact
/// columns floor at the wider of header and (capped) body token widths.
fn preferred_floor(metrics: &ColumnMetrics) -> usize {
    let token_target = match metrics.kind {
        ColumnKind::Narrative | ColumnKind::TokenHeavy => 16,
        ColumnKind::Compact => metrics
            .header_token_width
            .max(metrics.body_token_width.min(16)),
    };
    token_target
        .max(MIN_COLUMN_WIDTH)
        .min(metrics.max_width.max(MIN_COLUMN_WIDTH))
}

/// Shrink columns in priority order (TokenHeavy, Narrative, Compact),
/// balancing slack within each kind via a binary-searched cap so
/// similarly-shaped columns stay even.
fn shrink_columns(
    widths: &mut [usize],
    floors: &[usize],
    metrics: &[ColumnMetrics],
    mut amount: usize,
) -> usize {
    for kind in [
        ColumnKind::TokenHeavy,
        ColumnKind::Narrative,
        ColumnKind::Compact,
    ] {
        let slack = |idx: usize, width: usize| -> usize {
            width.saturating_sub(floors.get(idx).copied().unwrap_or(0))
        };
        let of_kind = |idx: usize| metrics.get(idx).is_some_and(|m| m.kind == kind);
        let slack_total: usize = widths
            .iter()
            .enumerate()
            .filter(|(idx, _)| of_kind(*idx))
            .map(|(idx, width)| slack(idx, *width))
            .sum();
        let to_remove = amount.min(slack_total);
        if to_remove == 0 {
            continue;
        }
        let mut low = 0_usize;
        let mut high = widths
            .iter()
            .enumerate()
            .filter(|(idx, _)| of_kind(*idx))
            .map(|(idx, width)| slack(idx, *width))
            .max()
            .unwrap_or(0);
        while low < high {
            let cap = low + (high - low) / 2;
            let removed: usize = widths
                .iter()
                .enumerate()
                .filter(|(idx, _)| of_kind(*idx))
                .map(|(idx, width)| slack(idx, *width).saturating_sub(cap))
                .sum();
            if removed > to_remove {
                low = cap + 1;
            } else {
                high = cap;
            }
        }
        let cap = low;
        let mut removed = 0_usize;
        for (idx, width) in widths.iter_mut().enumerate() {
            if !of_kind(idx) {
                continue;
            }
            let reduction = slack(idx, *width).saturating_sub(cap);
            *width -= reduction;
            removed += reduction;
        }
        let mut remainder = to_remove - removed;
        for (idx, width) in widths.iter_mut().enumerate() {
            if remainder == 0 {
                break;
            }
            if of_kind(idx) && slack(idx, *width) == cap && *width > 0 {
                *width -= 1;
                remainder -= 1;
            }
        }
        amount -= to_remove;
        if amount == 0 {
            break;
        }
    }
    amount
}

fn compute_column_widths(metrics: &[ColumnMetrics], available: usize) -> Option<Vec<usize>> {
    let mut widths: Vec<usize> = metrics
        .iter()
        .map(|m| m.max_width.max(MIN_COLUMN_WIDTH))
        .collect();
    let minimum_total = metrics.len() * MIN_COLUMN_WIDTH;
    if available < minimum_total {
        return None;
    }
    let mut floors: Vec<usize> = metrics.iter().map(preferred_floor).collect();
    let floor_total: usize = floors.iter().sum();
    if floor_total > available {
        let minimums = vec![MIN_COLUMN_WIDTH; floors.len()];
        shrink_columns(&mut floors, &minimums, metrics, floor_total - available);
    }
    let total: usize = widths.iter().sum();
    if total > available {
        let remaining = shrink_columns(&mut widths, &floors, metrics, total - available);
        if remaining > 0 {
            return None;
        }
    }
    Some(widths)
}

fn wrap_cell(cell: &TableCell, width: usize) -> Vec<Line<'static>> {
    if cell.lines.is_empty() {
        return vec![Line::default()];
    }
    let mut wrapped = Vec::new();
    for source in &cell.lines {
        let rendered = wrap_line(source, width.max(1), "");
        if rendered.is_empty() {
            wrapped.push(Line::default());
        } else {
            wrapped.extend(rendered);
        }
    }
    if wrapped.is_empty() {
        wrapped.push(Line::default());
    }
    wrapped
}

fn separator(widths: &[usize], ch: char, style: Style) -> Line<'static> {
    let gap = " ".repeat(TABLE_COLUMN_GAP);
    let text = widths
        .iter()
        .map(|w| ch.to_string().repeat(w + TABLE_CELL_PADDING * 2))
        .collect::<Vec<_>>()
        .join(&gap);
    Line::from(Span::styled(text, style))
}

fn render_row(
    row: &[TableCell],
    widths: &[usize],
    alignments: &[Alignment],
    row_style: Style,
) -> Vec<Line<'static>> {
    let wrapped: Vec<Vec<Line<'static>>> = row
        .iter()
        .zip(widths)
        .map(|(cell, width)| wrap_cell(cell, *width))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
    let mut out = Vec::with_capacity(height);
    for row_line in 0..height {
        let last_visible = wrapped
            .iter()
            .rposition(|lines| lines.get(row_line).is_some_and(|l| line_width(l) > 0));
        let Some(last_visible) = last_visible else {
            out.push(Line::default());
            continue;
        };
        let mut spans = Vec::new();
        for (column, width) in widths.iter().enumerate().take(last_visible + 1) {
            spans.push(Span::raw(" ".repeat(TABLE_CELL_PADDING)));
            let line = wrapped
                .get(column)
                .and_then(|lines| lines.get(row_line))
                .cloned()
                .unwrap_or_default();
            let remaining = width.saturating_sub(line_width(&line));
            let align = alignments.get(column).copied().unwrap_or(Alignment::None);
            let (left, right) = match align {
                Alignment::Left | Alignment::None => (0, remaining),
                Alignment::Center => (remaining / 2, remaining - remaining / 2),
                Alignment::Right => (remaining, 0),
            };
            if left > 0 {
                spans.push(Span::raw(" ".repeat(left)));
            }
            let mut styled = line;
            for span in &mut styled.spans {
                span.style = row_style.patch(span.style);
            }
            spans.extend(styled.spans);
            let last = column == last_visible;
            if !last {
                if right > 0 {
                    spans.push(Span::raw(" ".repeat(right)));
                }
                spans.push(Span::raw(" ".repeat(TABLE_CELL_PADDING + TABLE_COLUMN_GAP)));
            }
        }
        out.push(Line::from(spans));
    }
    out
}

fn expansive_cells_are_starved(
    row: &[TableCell],
    widths: &[usize],
    metrics: &[ColumnMetrics],
) -> bool {
    let expansive: Vec<(ColumnKind, usize, usize)> = row
        .iter()
        .zip(widths)
        .zip(metrics)
        .filter(|&((_, _), m)| m.kind != ColumnKind::Compact)
        .map(|((cell, width), m)| (m.kind, *width, wrap_cell(cell, *width).len()))
        .collect();
    expansive
        .iter()
        .filter(|(_, _, height)| *height >= CRAMPED_EXPANSIVE_CELL_LINES)
        .count()
        >= 2
        || expansive.iter().any(|(kind, width, height)| {
            *kind == ColumnKind::Narrative
                && *width < MIN_SCANNABLE_NARRATIVE_WIDTH
                && *height >= CATASTROPHIC_NARRATIVE_CELL_LINES
        })
}

/// Switch to key/value records once enough rows hold values the grid can no
/// longer present in useful chunks.
fn should_render_records(
    rows: &[Vec<TableCell>],
    widths: &[usize],
    metrics: &[ColumnMetrics],
) -> bool {
    if rows.is_empty() {
        return false;
    }
    let affected = rows
        .iter()
        .filter(|row| {
            let fragmented = row
                .iter()
                .zip(widths)
                .zip(metrics)
                .any(|((cell, width), m)| {
                    let has_fragmented_token = cell
                        .plain_text()
                        .split_whitespace()
                        .any(|token| token.width() > *width);
                    match m.kind {
                        ColumnKind::Compact => has_fragmented_token,
                        ColumnKind::TokenHeavy => {
                            *width < MIN_SCANNABLE_TOKEN_HEAVY_WIDTH && has_fragmented_token
                        }
                        ColumnKind::Narrative => false,
                    }
                });
            fragmented || expansive_cells_are_starved(row, widths, metrics)
        })
        .count();
    let threshold = if rows.len() == 1 {
        1
    } else {
        2.max(rows.len().div_ceil(3))
    };
    affected >= threshold
}

fn render_records(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    metrics: &[ColumnMetrics],
    available: usize,
    label_style: Style,
    separator_style: Style,
) -> Vec<Line<'static>> {
    let label_width = header
        .iter()
        .map(|h| h.plain_text().width())
        .max()
        .unwrap_or(0);
    let minimum_value = if metrics.iter().any(|m| m.kind != ColumnKind::Compact) {
        MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH
    } else {
        MIN_ALIGNED_COMPACT_VALUE_WIDTH
    };
    let aligned = FIELD_LEADING_PADDING + label_width + FIELD_GAP + minimum_value <= available;
    let mut out = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        for (head, value) in header.iter().zip(row) {
            if aligned {
                let indent = FIELD_LEADING_PADDING + label_width + FIELD_GAP;
                let value_width = available.saturating_sub(indent).max(MIN_VALUE_WIDTH);
                for (line_index, value_line) in
                    wrap_cell(value, value_width).into_iter().enumerate()
                {
                    let mut spans = Vec::new();
                    if line_index == 0 {
                        let label = head.plain_text();
                        spans.push(Span::raw(" ".repeat(FIELD_LEADING_PADDING)));
                        spans.push(Span::styled(label.clone(), label_style));
                        spans.push(Span::raw(
                            " ".repeat(label_width.saturating_sub(label.width()) + FIELD_GAP),
                        ));
                    } else {
                        spans.push(Span::raw(" ".repeat(indent)));
                    }
                    spans.extend(value_line.spans);
                    out.push(Line::from(spans));
                }
            } else {
                let label = Line::from(Span::styled(head.plain_text(), label_style));
                for label_line in wrap_line(
                    &label,
                    available.saturating_sub(FIELD_LEADING_PADDING).max(1),
                    "",
                ) {
                    let mut spans = vec![Span::raw(" ".repeat(FIELD_LEADING_PADDING))];
                    spans.extend(label_line.spans);
                    out.push(Line::from(spans));
                }
                let value_width = available.saturating_sub(STACKED_VALUE_INDENT).max(1);
                for value_line in wrap_cell(value, value_width) {
                    let mut spans = vec![Span::raw(" ".repeat(STACKED_VALUE_INDENT))];
                    spans.extend(value_line.spans);
                    out.push(Line::from(spans));
                }
            }
        }
        if row_index + 1 < rows.len() {
            out.push(Line::from(Span::styled(
                TABLE_BODY_SEPARATOR_CHAR
                    .to_string()
                    .repeat(available.min(60)),
                separator_style,
            )));
        }
    }
    out
}

/// The full pipeline: spillover filtering, normalization, width allocation,
/// grid-vs-records choice, spillover appended as plain text.
pub fn render(
    header: Vec<TableCell>,
    body: Vec<Vec<TableCell>>,
    alignments: &[Alignment],
    available: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let columns = alignments.len().max(header.len());
    if columns == 0 {
        return Vec::new();
    }
    let mut spillover: Vec<TableCell> = Vec::new();
    let mut rows: Vec<Vec<TableCell>> = Vec::new();
    for row in body {
        if columns > 1 && row.len() == 1 {
            spillover.extend(row);
        } else {
            rows.push(row);
        }
    }
    let mut header = header;
    header.resize(columns, TableCell::default());
    for row in &mut rows {
        row.truncate(columns);
        row.resize(columns, TableCell::default());
    }
    let metrics = collect_metrics(&header, &rows, columns);
    let reserved = columns.saturating_sub(1) * TABLE_COLUMN_GAP + columns * TABLE_CELL_PADDING * 2;
    let content_budget = available.saturating_sub(reserved);
    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let separator_style = theme.dim_style();

    let mut out = Vec::new();
    match compute_column_widths(&metrics, content_budget) {
        Some(widths) if !should_render_records(&rows, &widths, &metrics) => {
            out.extend(render_row(&header, &widths, alignments, header_style));
            out.push(separator(
                &widths,
                TABLE_HEADER_SEPARATOR_CHAR,
                separator_style,
            ));
            for (index, row) in rows.iter().enumerate() {
                out.extend(render_row(row, &widths, alignments, Style::default()));
                if index + 1 < rows.len() {
                    out.push(separator(
                        &widths,
                        TABLE_BODY_SEPARATOR_CHAR,
                        separator_style,
                    ));
                }
            }
        }
        _ => {
            out.extend(render_records(
                &header,
                &rows,
                &metrics,
                available,
                header_style,
                separator_style,
            ));
        }
    }
    for cell in spillover {
        out.push(Line::default());
        out.extend(cell.lines);
    }
    out
}
