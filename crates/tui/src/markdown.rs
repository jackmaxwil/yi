use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

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
    alignments: Vec<Alignment>,
    header: Vec<crate::table::TableCell>,
    rows: Vec<Vec<crate::table::TableCell>>,
    current: Vec<crate::table::TableCell>,
    in_cell: bool,
    in_header: bool,
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
        if self.table.is_some() {
            let style = self.style();
            if let Some(table) = &mut self.table
                && table.in_cell
                && let Some(cell) = table.current.last_mut()
            {
                cell.push_span(Span::styled(text.to_owned(), style));
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

fn reduce_inline<'e>(b: &mut Builder, event: Event<'e>) -> Option<Event<'e>> {
    match event {
        Event::Start(Tag::Emphasis) => b.push_style(|s| s.add_modifier(Modifier::ITALIC)),
        Event::End(TagEnd::Emphasis) => b.pop_style(),
        Event::Start(Tag::Strong) => b.push_style(|s| s.add_modifier(Modifier::BOLD)),
        Event::End(TagEnd::Strong) => b.pop_style(),
        Event::Start(Tag::Strikethrough) => {
            b.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT));
        }
        Event::End(TagEnd::Strikethrough) => b.pop_style(),
        Event::Start(Tag::Link { .. }) => {
            b.push_style(|s| s.add_modifier(Modifier::UNDERLINED));
            b.text("");
        }
        Event::End(TagEnd::Link) => b.pop_style(),
        Event::Code(code) => {
            let style = Style::default()
                .fg(b.theme.accent)
                .add_modifier(Modifier::BOLD);
            if let Some(table) = &mut b.table {
                if table.in_cell
                    && let Some(cell) = table.current.last_mut()
                {
                    cell.push_span(Span::styled(code.into_string(), style));
                }
                return None;
            }
            if b.spans.is_empty() && !b.indent.is_empty() {
                let indent = b.indent.clone();
                b.spans.push(Span::raw(indent));
            }
            b.spans.push(Span::styled(code.into_string(), style));
        }
        other => return Some(other),
    }
    None
}

fn reduce_table(b: &mut Builder, event: &Event) -> bool {
    match event {
        Event::Start(Tag::Table(alignments)) => {
            b.blank();
            b.table = Some(TableState {
                alignments: alignments.clone(),
                ..TableState::default()
            });
        }
        Event::Start(Tag::TableHead) => {
            if let Some(table) = &mut b.table {
                table.in_header = true;
                table.current = Vec::new();
            }
        }
        Event::Start(Tag::TableRow) => {
            if let Some(table) = &mut b.table {
                table.current = Vec::new();
            }
        }
        Event::End(TagEnd::TableHead) => {
            if let Some(table) = &mut b.table {
                table.header = std::mem::take(&mut table.current);
                table.in_header = false;
            }
        }
        Event::End(TagEnd::TableRow) => {
            if let Some(table) = &mut b.table {
                let row = std::mem::take(&mut table.current);
                table.rows.push(row);
            }
        }
        Event::Start(Tag::TableCell) => {
            if let Some(table) = &mut b.table {
                table.current.push(crate::table::TableCell::default());
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
                let available = b.width.saturating_sub(b.indent.len());
                let rendered = crate::table::render(
                    table.header,
                    table.rows,
                    &table.alignments,
                    available,
                    b.theme,
                );
                let indent = b.indent.clone();
                for mut line in rendered {
                    line.spans.insert(0, Span::raw(indent.clone()));
                    b.out.push(line);
                }
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
        let Some(event) = reduce_inline(&mut b, event) else {
            continue;
        };
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
            event if reduce_table(&mut b, &event) => {}
            Event::Text(text) => b.text(&text),
            Event::SoftBreak => b.text(" "),
            Event::HardBreak => {
                if let Some(table) = &mut b.table {
                    if table.in_cell
                        && let Some(cell) = table.current.last_mut()
                    {
                        cell.hard_break();
                    }
                } else {
                    b.flush_line();
                }
            }
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
