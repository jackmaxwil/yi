use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::colors::Theme;
use crate::wrap::wrap_line;

/// The newline gate (codex `markdown_stream.rs:82-96`, verbatim semantics):
/// the longest prefix of `source` that ends at a newline is safe to commit;
/// the remainder may still change while the model streams.
pub fn commit_complete_source(source: &str) -> (&str, &str) {
    match source.rfind('\n') {
        Some(index) => source.split_at(index + 1),
        None => ("", source),
    }
}

/// The streaming commit point (U13): the longest prefix ending at a blank
/// line outside any code fence. Prefix markdown re-renders identically as
/// the source grows, so its lines can be committed to scrollback while the
/// tail keeps streaming.
pub fn stable_cut(source: &str) -> usize {
    let mut cut = 0;
    let mut offset = 0;
    let mut in_fence = false;
    let mut previous_blank = false;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && trimmed.is_empty() && !previous_blank && offset > 0 {
            cut = offset + line.len();
        }
        previous_blank = trimmed.is_empty();
        offset += line.len();
    }
    cut
}

struct Builder<'t> {
    theme: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    indent: String,
    list_stack: Vec<Option<u64>>,
    in_code_block: bool,
    table: Option<TableState>,
}

#[derive(Default)]
struct TableState {
    rows: Vec<Vec<String>>,
    current: Vec<String>,
    in_cell: bool,
}

impl Builder<'_> {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, apply: impl Fn(Style) -> Style) {
        self.styles.push(apply(self.style()));
    }

    fn pop_style(&mut self) {
        self.styles.pop();
    }

    fn flush_line(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let line = Line::from(std::mem::take(&mut self.spans));
        let indent = format!("{}  ", self.indent);
        self.out.extend(wrap_line(&line, self.width, &indent));
    }

    fn blank(&mut self) {
        self.flush_line();
        if !self.out.last().is_some_and(|l| l.spans.is_empty()) && !self.out.is_empty() {
            self.out.push(Line::default());
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(table) = &mut self.table {
            if table.in_cell
                && let Some(cell) = table.current.last_mut()
            {
                cell.push_str(text);
            }
            return;
        }
        if self.in_code_block {
            for raw in text.split_inclusive('\n') {
                let chunk = raw.strip_suffix('\n');
                let body = chunk.unwrap_or(raw);
                self.spans.push(Span::styled(
                    format!("{}{body}", self.indent),
                    self.theme.dim_style(),
                ));
                if chunk.is_some() {
                    let line = Line::from(std::mem::take(&mut self.spans));
                    self.out.push(line);
                }
            }
            return;
        }
        if self.spans.is_empty() && !self.indent.is_empty() {
            self.spans.push(Span::raw(self.indent.clone()));
        }
        self.spans.push(Span::styled(text.to_owned(), self.style()));
    }
}

/// Minimal fixed-layout table: column widths from content (capped), header
/// bold, rules dim. Cells truncate rather than wrap (D41 polish; codex ships
/// no table engine, opencode gets one free from OpenTUI — this is the ~60
/// lines that cover agent output tables).
fn render_table(b: &mut Builder, rows: &[Vec<String>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }
    let available = b.width.saturating_sub(b.indent.len() + 2);
    let cap = (available / columns).saturating_sub(3).clamp(4, 32);
    let mut widths = vec![1_usize; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(slot) = widths.get_mut(i) {
                *slot = (*slot).max(cell.trim().width().min(cap));
            }
        }
    }
    let clip = |text: &str, max: usize| -> String {
        let text = text.trim();
        if text.width() <= max {
            format!("{text}{}", " ".repeat(max - text.width()))
        } else {
            let mut out = String::new();
            let mut used = 0;
            for ch in text.chars() {
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + w > max.saturating_sub(1) {
                    break;
                }
                used += w;
                out.push(ch);
            }
            format!("{out}…{}", " ".repeat(max.saturating_sub(used + 1)))
        }
    };
    for (index, row) in rows.iter().enumerate() {
        let mut spans = vec![Span::raw(b.indent.clone())];
        for (i, width) in widths.iter().enumerate() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            let style = if index == 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                b.style()
            };
            spans.push(Span::styled(clip(cell, *width), style));
            if i + 1 < columns {
                spans.push(Span::styled(" │ ".to_owned(), b.theme.dim_style()));
            }
        }
        b.out.push(Line::from(spans));
        if index == 0 {
            let rule: String = widths
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let bar = "─".repeat(*w);
                    if i + 1 < columns {
                        format!("{bar}─┼─")
                    } else {
                        bar
                    }
                })
                .collect();
            b.out.push(Line::from(vec![
                Span::raw(b.indent.clone()),
                Span::styled(rule, b.theme.dim_style()),
            ]));
        }
    }
}

fn reduce_table(b: &mut Builder, event: &Event) -> bool {
    match event {
        Event::Start(Tag::Table(_)) => {
            b.blank();
            b.table = Some(TableState::default());
        }
        Event::Start(Tag::TableHead | Tag::TableRow) => {
            if let Some(table) = &mut b.table {
                table.current = Vec::new();
            }
        }
        Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
            if let Some(table) = &mut b.table {
                let row = std::mem::take(&mut table.current);
                table.rows.push(row);
            }
        }
        Event::Start(Tag::TableCell) => {
            if let Some(table) = &mut b.table {
                table.current.push(String::new());
                table.in_cell = true;
            }
        }
        Event::End(TagEnd::TableCell) => {
            if let Some(table) = &mut b.table {
                table.in_cell = false;
            }
        }
        Event::End(TagEnd::Table) => {
            if let Some(table) = b.table.take() {
                render_table(b, &table.rows);
            }
        }
        _ => return false,
    }
    true
}

pub fn render(source: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut b = Builder {
        theme,
        width,
        out: Vec::new(),
        spans: Vec::new(),
        styles: vec![Style::default().fg(theme.text)],
        indent: "  ".to_owned(),
        list_stack: Vec::new(),
        in_code_block: false,
        table: None,
    };
    let parser = Parser::new_ext(
        source,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    );
    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                b.blank();
                b.push_style(|s| s.add_modifier(Modifier::BOLD));
            }
            Event::End(TagEnd::Heading(_)) => {
                b.pop_style();
                b.blank();
            }
            Event::Start(Tag::Paragraph) => b.blank(),
            Event::End(TagEnd::Paragraph) => b.flush_line(),
            Event::Start(Tag::BlockQuote(_)) => {
                b.blank();
                b.indent.push_str("▌ ");
                b.push_style(|s| s.add_modifier(Modifier::ITALIC));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                b.pop_style();
                let len = b.indent.len().saturating_sub("▌ ".len());
                b.indent.truncate(len);
                b.flush_line();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                b.blank();
                let fence = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        format!("{}```{lang}", b.indent)
                    }
                    _ => format!("{}```", b.indent),
                };
                b.out
                    .push(Line::from(Span::styled(fence, b.theme.dim_style())));
                b.in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                b.flush_line();
                let fence = format!("{}```", b.indent);
                b.out
                    .push(Line::from(Span::styled(fence, b.theme.dim_style())));
                b.in_code_block = false;
            }
            Event::Start(Tag::List(start)) => {
                if b.list_stack.is_empty() {
                    b.blank();
                }
                b.list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                b.list_stack.pop();
                if b.list_stack.is_empty() {
                    b.flush_line();
                }
            }
            Event::Start(Tag::Item) => {
                b.flush_line();
                let depth = b.list_stack.len().saturating_sub(1);
                let pad = "  ".repeat(depth);
                let marker = match b.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n = n.saturating_add(1);
                        marker
                    }
                    _ => "- ".to_owned(),
                };
                b.spans.push(Span::styled(
                    format!("{}{pad}{marker}", b.indent),
                    b.style(),
                ));
            }
            Event::End(TagEnd::Item) => b.flush_line(),
            Event::Start(Tag::Emphasis) => b.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Event::End(TagEnd::Emphasis) => b.pop_style(),
            Event::Start(Tag::Strong) => b.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Event::End(TagEnd::Strong) => b.pop_style(),
            Event::Start(Tag::Strikethrough) => {
                b.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT));
            }
            Event::End(TagEnd::Strikethrough) => b.pop_style(),
            Event::Start(Tag::Link { dest_url, .. }) => {
                b.push_style(|s| s.add_modifier(Modifier::UNDERLINED));
                b.text("");
                let _ = dest_url;
            }
            Event::End(TagEnd::Link) => b.pop_style(),
            Event::Code(code) => {
                let style = Style::default()
                    .fg(b.theme.accent)
                    .add_modifier(Modifier::BOLD);
                if b.spans.is_empty() && !b.indent.is_empty() {
                    let indent = b.indent.clone();
                    b.spans.push(Span::raw(indent));
                }
                b.spans.push(Span::styled(code.into_string(), style));
            }
            event if reduce_table(&mut b, &event) => {}
            Event::Text(text) => b.text(&text),
            Event::SoftBreak => b.text(" "),
            Event::HardBreak => b.flush_line(),
            Event::Rule => {
                b.blank();
                let fill: String =
                    std::iter::repeat_n('─', width.saturating_sub(4).min(40)).collect();
                b.out.push(Line::from(Span::styled(
                    format!("{}{fill}", b.indent),
                    b.theme.dim_style(),
                )));
            }
            _ => {}
        }
    }
    b.flush_line();
    while b.out.first().is_some_and(|l| l.spans.is_empty()) {
        b.out.remove(0);
    }
    while b.out.last().is_some_and(|l| l.spans.is_empty()) {
        b.out.pop();
    }
    b.out
}
