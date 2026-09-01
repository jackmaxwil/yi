use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::colors::Theme;
use crate::wrap::wrap_line;

const CODE_RAIL: &str = "│";
const CODE_RAIL_INDENT: &str = "│ ";

/// Where the streamed source is safe to commit, and how to render a slice
/// that starts past the cut.
pub struct StableStream {
    /// Byte offset: everything before re-renders identically as the source
    /// grows.
    pub cut: usize,
    /// Set when `cut` sits inside a top-level fence: the fence's opening
    /// line, prepended when rendering a slice that starts at `cut` so its
    /// rows still render as code.
    pub reopen: Option<String>,
}

/// U13 commit gate. Outside a fence only a blank line is stable (paragraphs
/// re-wrap, lists renumber); inside a top-level fence every completed line
/// is — matched by marker char and run length; indented fences stay opaque.
pub fn stable_stream(source: &str) -> StableStream {
    let mut cut = 0;
    let mut reopen = None;
    let mut offset = 0;
    // (marker, open run length, top level, opening line)
    let mut fence: Option<(char, usize, bool, String)> = None;
    let mut previous_blank = false;
    for line in source.split_inclusive('\n') {
        let complete = line.ends_with('\n');
        let trimmed = line.trim();
        let run = |marker: char| trimmed.chars().take_while(|ch| *ch == marker).count();
        match &fence {
            Some((marker, open_len, top, open_line)) => {
                let closes = run(*marker) >= *open_len && trimmed.chars().all(|ch| ch == *marker);
                if closes && complete {
                    if *top {
                        cut = offset + line.len();
                        reopen = None;
                    }
                    fence = None;
                } else if *top && complete {
                    cut = offset + line.len();
                    reopen = Some(open_line.clone());
                }
            }
            None => {
                let backticks = run('`');
                let tildes = run('~');
                if backticks >= 3 || tildes >= 3 {
                    let (marker, open_len) = if backticks >= 3 {
                        ('`', backticks)
                    } else {
                        ('~', tildes)
                    };
                    let top = line.starts_with(marker);
                    let open_line = line.trim_end_matches('\n').to_owned();
                    if top && complete {
                        cut = offset + line.len();
                        reopen = Some(open_line.clone());
                    }
                    fence = Some((marker, open_len, top, open_line));
                } else if trimmed.is_empty() && !previous_blank && offset > 0 && complete {
                    cut = offset + line.len();
                    reopen = None;
                }
            }
        }
        previous_blank = trimmed.is_empty();
        offset += line.len();
    }
    StableStream { cut, reopen }
}

struct Builder<'t> {
    theme: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    indent: String,
    list_stack: Vec<ListLevel>,
    pending_marker: Option<Span<'static>>,
    in_code_block: bool,
    continued: bool,
    code_lang: &'t mut Option<crate::highlight::Lang>,
    link_dest: Option<String>,
    table: Option<TableState>,
}

struct ListLevel {
    next: Option<u64>,
    hang: usize,
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
        // The hang is the width the markers actually occupy: a fixed two spaces
        // left `10. ` and every nested glyph wrapping two columns short.
        let hang: usize = self.list_stack.iter().map(|level| level.hang).sum();
        let indent = format!("{}{}", self.indent, " ".repeat(hang));
        self.out.extend(wrap_line(&line, self.width, &indent));
    }

    // The marker is held rather than written so that whichever line opens the
    // item claims it. Writing it eagerly let the paragraph a loose list wraps
    // its items in flush the marker alone, one blank line above its own text.
    fn line_prologue(&mut self) {
        if !self.spans.is_empty() {
            return;
        }
        if !self.indent.is_empty() {
            self.spans.push(Span::raw(self.indent.clone()));
        }
        if let Some(marker) = self.pending_marker.take() {
            self.spans.push(marker);
        }
    }

    fn blank(&mut self) {
        self.flush_line();
        if !self.out.last().is_some_and(|l| l.spans.is_empty()) && !self.out.is_empty() {
            self.out.push(Line::default());
        }
    }

    fn start_item(&mut self) {
        self.flush_line();
        let depth = self.list_stack.len().saturating_sub(1);
        let pad: usize = self
            .list_stack
            .iter()
            .rev()
            .skip(1)
            .map(|level| level.hang)
            .sum();
        let marker = match self.list_stack.last_mut() {
            Some(ListLevel {
                next: Some(index), ..
            }) => {
                let marker = format!("{index}. ");
                *index = index.saturating_add(1);
                marker
            }
            // OMP `md.bullet`, with depth glyphs so nesting reads.
            _ => match depth {
                0 => "• ".to_owned(),
                1 => "◦ ".to_owned(),
                _ => "‣ ".to_owned(),
            },
        };
        if let Some(level) = self.list_stack.last_mut() {
            level.hang = UnicodeWidthStr::width(marker.as_str());
        }
        let style = self.style();
        self.pending_marker = Some(Span::styled(format!("{}{marker}", " ".repeat(pad)), style));
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
            let base = self.theme.dim_style();
            for raw in text.split_inclusive('\n') {
                let chunk = raw.strip_suffix('\n');
                let body = chunk.unwrap_or(raw);
                self.spans
                    .push(Span::styled(self.indent.clone(), self.theme.dim_style()));
                match self.code_lang.as_mut() {
                    Some(lang) => self
                        .spans
                        .extend(crate::highlight::spans(body, lang, self.theme, base)),
                    None => self.spans.push(Span::styled(body.to_owned(), base)),
                }
                if chunk.is_some() {
                    let line = Line::from(std::mem::take(&mut self.spans));
                    self.out.push(line);
                }
            }
            return;
        }
        self.line_prologue();
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
        Event::Start(Tag::Link { dest_url, .. }) => {
            b.link_dest = Some(dest_url.to_string());
            b.push_style(|s| s.fg(b.theme.accent).add_modifier(Modifier::UNDERLINED));
            b.text("");
        }
        Event::End(TagEnd::Link) => {
            b.pop_style();
            // codex appends the destination after the label; a label alone
            // drops the only information a link carries.
            if let Some(dest) = b.link_dest.take()
                && !dest.is_empty()
            {
                let style = b.theme.dim_style();
                b.spans.push(Span::styled(format!(" ({dest})"), style));
            }
        }
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
            b.line_prologue();
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
    render_stream(source, width, theme, false, &mut None)
}
/// `continued` reopens a fence already on screen: its rail header is drawn once
/// per block, and `lang` carries the syntax parse state across the commit seam.
pub fn render_stream(
    source: &str,
    width: usize,
    theme: &Theme,
    continued: bool,
    lang: &mut Option<crate::highlight::Lang>,
) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut b = Builder {
        theme,
        width,
        out: Vec::new(),
        spans: Vec::new(),
        styles: vec![Style::default().fg(theme.text)],
        indent: String::new(),
        list_stack: Vec::new(),
        pending_marker: None,
        in_code_block: false,
        continued,
        code_lang: lang,
        link_dest: None,
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
            Event::Start(Tag::Heading { level, .. }) => {
                b.blank();
                let accent = b.theme.accent;
                // codex's ladder (`markdown_render.rs:107-125`): the level has
                // to be legible without the literal `#` marks Yi drops.
                b.push_style(move |s| match level {
                    HeadingLevel::H1 => s
                        .fg(accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    HeadingLevel::H2 => s.fg(accent).add_modifier(Modifier::BOLD),
                    HeadingLevel::H3 => s.add_modifier(Modifier::BOLD | Modifier::ITALIC),
                    _ => s.add_modifier(Modifier::ITALIC),
                });
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
                let continued = std::mem::take(&mut b.continued);
                if b.pending_marker.is_some() {
                    b.line_prologue();
                    b.flush_line();
                }
                b.blank();
                // OMP gives fenced code a border hook rather than the author's
                // backticks; only a fence has an opening line a stream reopens.
                if let CodeBlockKind::Fenced(lang) = &kind {
                    if !continued {
                        *b.code_lang = crate::highlight::lang_for(lang);
                        let head = format!("{}{CODE_RAIL} {lang}", b.indent);
                        b.out.push(Line::from(Span::styled(
                            head.trim_end().to_owned(),
                            b.theme.dim_style(),
                        )));
                    }
                } else if !continued {
                    *b.code_lang = None;
                }
                b.indent.push_str(CODE_RAIL_INDENT);
                b.in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                b.flush_line();
                let len = b.indent.len().saturating_sub(CODE_RAIL_INDENT.len());
                b.indent.truncate(len);
                b.in_code_block = false;
            }
            Event::Start(Tag::List(start)) => {
                if b.list_stack.is_empty() {
                    b.blank();
                }
                b.list_stack.push(ListLevel {
                    next: start,
                    hang: 0,
                });
            }
            Event::End(TagEnd::List(_)) => {
                b.list_stack.pop();
                if b.list_stack.is_empty() {
                    b.flush_line();
                }
            }
            Event::Start(Tag::Item) => b.start_item(),
            Event::End(TagEnd::Item) => {
                if b.pending_marker.is_some() {
                    b.line_prologue();
                }
                b.flush_line();
            }
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
                b.out.push(Line::from(Span::styled(
                    format!("{}———", b.indent),
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
