use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
    width: usize,
}

fn flatten(line: &Line<'_>) -> Vec<Cell> {
    let mut cells = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            cells.push(Cell {
                ch,
                style: span.style,
                width: UnicodeWidthChar::width(ch).unwrap_or(0),
            });
        }
    }
    cells
}

fn rebuild(cells: &[Cell], indent: Option<&str>) -> Line<'static> {
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

fn token_is_unbreakable(cells: &[Cell]) -> bool {
    let text: String = cells.iter().map(|c| c.ch).collect();
    text.contains("://")
}

/// Word wrap over spans, unicode-width aware (design U14). Breaks at spaces;
/// an overlong plain token splits at a character boundary, but a token
/// containing `://` is never split — the line overflows instead so terminal
/// link detection keeps seeing one intact token (codex wrapping.rs, the idea).
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
        if *word_width > max && !token_is_unbreakable(word) {
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
            // A space with nothing before it on the very first line is real
            // indentation (a nested list marker, a padded cell), not a word
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

pub fn wrap_lines(lines: &[Line<'_>], width: usize, subsequent_indent: &str) -> Vec<Line<'static>> {
    lines
        .iter()
        .flat_map(|line| wrap_line(line, width, subsequent_indent))
        .collect()
}
