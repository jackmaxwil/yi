use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) style: Style,
    pub(crate) width: usize,
}

/// One row: a line wider than `width` keeps what fits before a closing `…`.
pub fn fit(line: Line<'static>, width: usize) -> Line<'static> {
    let cells = flatten(&line);
    if cells.iter().map(|cell| cell.width).sum::<usize>() <= width {
        return line;
    }
    let kept = take_cells(&cells, width.saturating_sub(1));
    let mut cut = rebuild(kept, None);
    let style = cells
        .get(kept.len())
        .map_or_else(Style::default, |cell| cell.style);
    cut.spans.push(Span::styled("…", style));
    Line {
        style: line.style,
        alignment: line.alignment,
        ..cut
    }
}

pub(crate) fn flatten_spans(spans: &[Span<'_>]) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut cluster = String::new();
    let mut cluster_width = 0usize;
    for span in spans {
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            let width = if width == 0 {
                cluster.push(ch);
                let measured = UnicodeWidthStr::width(cluster.as_str());
                let delta = measured.saturating_sub(cluster_width);
                cluster_width = measured;
                delta
            } else {
                cluster.clear();
                cluster.push(ch);
                cluster_width = width;
                width
            };
            cells.push(Cell {
                ch,
                style: span.style,
                width,
            });
        }
    }
    cells
}

fn flatten(line: &Line<'_>) -> Vec<Cell> {
    flatten_spans(&line.spans)
}

pub(crate) fn take_cells(cells: &[Cell], budget: usize) -> &[Cell] {
    let mut used = 0usize;
    let end = cells
        .iter()
        .take_while(|cell| {
            used = used.saturating_add(cell.width);
            used <= budget
        })
        .count();
    cells.get(..end).unwrap_or(cells)
}

pub(crate) fn rebuild(cells: &[Cell], indent: Option<&str>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let Some(indent) = indent
        && !indent.is_empty()
    {
        spans.push(Span::raw(indent.to_owned()));
    }
    let mut current = String::new();
    let mut style = Style::default();
    for cell in cells {
        if current.is_empty() {
            style = cell.style;
        } else if cell.style != style {
            spans.push(Span::styled(std::mem::take(&mut current), style));
            style = cell.style;
        }
        current.push(cell.ch);
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, style));
    }
    Line::from(spans)
}

/// Breaks at spaces; a token wider than the row splits at a character boundary, a URL too:
/// ratatui clips every row at the buffer width, so an overflowing URL was cut short anyway.
pub fn wrap_line(line: &Line<'_>, width: usize, subsequent_indent: &str) -> Vec<Line<'static>> {
    let width = width.max(1);
    let indent_width: usize = subsequent_indent
        .chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    let cont_width = width.saturating_sub(indent_width).max(1);
    let cells = flatten(line);
    let mut out: Vec<Vec<Cell>> = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    let mut current_width = 0_usize;
    let mut word: Vec<Cell> = Vec::new();
    let mut word_width = 0_usize;

    let limit = |out: &[Vec<Cell>]| if out.is_empty() { width } else { cont_width };

    let flush_word = |out: &mut Vec<Vec<Cell>>,
                      current: &mut Vec<Cell>,
                      current_width: &mut usize,
                      word: &mut Vec<Cell>,
                      word_width: &mut usize| {
        if word.is_empty() {
            return;
        }
        if *current_width + *word_width > limit(out) && !current.is_empty() {
            out.push(std::mem::take(current));
            *current_width = 0;
        }
        let max = limit(out);
        if *word_width > max {
            for cell in word.drain(..) {
                if *current_width + cell.width > limit(out) && !current.is_empty() {
                    out.push(std::mem::take(current));
                    *current_width = 0;
                }
                *current_width += cell.width;
                current.push(cell);
            }
        } else {
            *current_width += *word_width;
            current.append(word);
        }
        *word_width = 0;
    };

    for cell in cells {
        if cell.ch == ' ' {
            flush_word(
                &mut out,
                &mut current,
                &mut current_width,
                &mut word,
                &mut word_width,
            );
            // A leading space on the first line is real indentation, not a word
            // separator — dropping it flattened every nested bullet.
            let leading_indent = current.is_empty() && out.is_empty();
            if (leading_indent || !current.is_empty()) && current_width < limit(&out) {
                current_width += 1;
                current.push(cell);
            }
        } else {
            word.push(cell);
            word_width += cell.width;
        }
    }
    flush_word(
        &mut out,
        &mut current,
        &mut current_width,
        &mut word,
        &mut word_width,
    );
    while current.last().is_some_and(|c| c.ch == ' ') {
        current.pop();
    }
    out.push(current);

    out.iter()
        .enumerate()
        .map(|(i, cells)| {
            let mut cells = cells.as_slice();
            while cells.first().is_some_and(|c| c.ch == ' ') && i > 0 {
                cells = cells.get(1..).unwrap_or(&[]);
            }
            rebuild(cells, (i > 0).then_some(subsequent_indent))
        })
        .collect()
}

pub fn hard_wrap(line: &Line<'_>, width: usize, indent: &Span<'static>) -> Vec<Line<'static>> {
    let width = width.max(1);
    let cont_width = width.saturating_sub(indent.width()).max(1);
    let mut out: Vec<Vec<Cell>> = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    let mut current_width = 0_usize;
    for cell in flatten(line) {
        let limit = if out.is_empty() { width } else { cont_width };
        if current_width + cell.width > limit && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current_width += cell.width;
        current.push(cell);
    }
    out.push(current);
    out.iter()
        .enumerate()
        .map(|(i, cells)| {
            let mut row = rebuild(cells, None);
            if i > 0 {
                row.spans.insert(0, indent.clone());
            }
            row
        })
        .collect()
}
