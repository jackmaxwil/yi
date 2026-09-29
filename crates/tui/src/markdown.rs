use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::colors::Theme;
use crate::wrap::{hard_wrap, wrap_line};

const CODE_RAIL_INDENT: &str = "│ ";
const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH.union(Options::ENABLE_TABLES);

/// Where the streamed source is safe to commit, and how to render a slice
/// that starts past the cut.
pub struct StableStream {
    /// Byte offset: everything before re-renders identically as the source
    /// grows.
    pub cut: usize,
    /// Set when [`StableStream::cut`] sits inside a fence: the opener (after a stub of each item
    /// around it) a slice from the cut renders under, less the rows that context draws alone.
    pub reopen: Option<String>,
    /// Where the last top-level block starts: nothing before it can change, so the next
    /// parse starts there.
    pub block: usize,
}

thread_local! {
    static WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Bytes this thread has scanned for cuts and rendered: what a streamed delta costs.
pub fn work() -> (usize, usize) {
    WORK.with(std::cell::Cell::get)
}

fn count(scanned: usize, rendered: usize) {
    WORK.with(|work| {
        let (was_scanned, was_rendered) = work.get();
        work.set((was_scanned + scanned, was_rendered + rendered));
    });
}

/// A character that may open a line's block marker.
pub(crate) fn lead(ch: char) -> bool {
    ch.is_ascii_digit()
        || matches!(
            ch,
            ' ' | '\t'
                | '-'
                | '*'
                | '+'
                | '_'
                | '#'
                | '>'
                | '='
                | '`'
                | '~'
                | '|'
                | ':'
                | '.'
                | ')'
        )
}

/// A block on an unfinished line of marker characters only: `1` may yet be `1. `, `-` a rule.
pub(crate) fn undecided(source: &str, at: usize) -> bool {
    source
        .get(at..)
        .is_some_and(|rest| !rest.contains('\n') && rest.chars().all(lead))
}

pub(crate) fn item_marker(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let end = if (1..=9).contains(&digits) {
        line.get(digits..)?
            .starts_with(['.', ')'])
            .then_some(digits + 1)?
    } else {
        line.starts_with(['-', '*', '+']).then_some(1)?
    };
    let rest = line.get(end..)?;
    (rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r'])).then(|| line.get(..end))?
}

/// The fence ending the source: where it is, whether an item or quote holds it, and the items
/// around it as (start, number shown, list loose).
struct OpenFence {
    range: std::ops::Range<usize>,
    contained: bool,
    around: Vec<(usize, Option<u64>, bool)>,
}

/// The §17.3 commit gate, read off the parser: a cut is stable where a new top-level block has
/// begun, or after a completed line of the fence the source ends in.
pub fn stable_stream(source: &str) -> StableStream {
    let (last_top, fence) = last_block(source);
    let top = line_start(source, last_top);
    let block = StableStream {
        cut: if undecided(source, top) { 0 } else { top },
        reopen: None,
        block: top,
    };
    let Some(fence) = fence else {
        return block;
    };
    let Some(opener_end) = source
        .get(fence.range.start..)
        .and_then(|rest| rest.find('\n'))
        .map(|nl| fence.range.start + nl + 1)
    else {
        return block;
    };
    let opener = source
        .get(line_start(source, fence.range.start)..opener_end)
        .unwrap_or_default();
    let (last_line, closed) =
        fence_lines(source, opener, opener_end, fence.range.end, fence.contained);
    if let Some(end) = closed
        && !fence.contained
    {
        return StableStream {
            cut: end,
            reopen: None,
            block: top,
        };
    }
    StableStream {
        cut: last_line.max(opener_end),
        reopen: Some(fence_context(source, &fence.around, opener)),
        block: top,
    }
}

/// [`stable_stream`] resumed where nothing before can change: the last top-level block, or the
/// last cut inside a top-level fence, re-parsed under its opener. A delta costs its own block.
#[derive(Default)]
pub struct StableScan {
    from: usize,
    opener: Option<String>,
    block: usize,
}

impl StableScan {
    pub fn scan(&mut self, source: &str) -> StableStream {
        if source.get(self.from..).is_none() {
            *self = Self::default();
        }
        let rest = source.get(self.from..).unwrap_or_default();
        count(rest.len(), 0);
        let (doc, head) = match &self.opener {
            Some(opener) => (format!("{opener}\n{rest}"), opener.len() + 1),
            None => (rest.to_owned(), 0),
        };
        let stream = stable_stream(&doc);
        let at = |offset: usize| (self.from + offset).saturating_sub(head);
        // A cut of 0 is an undecided block, which may yet join the block before it.
        let decided = stream.cut > 0 || !undecided(&doc, stream.block);
        let cut = if decided { at(stream.cut) } else { 0 };
        if decided && stream.block >= head {
            self.block = at(stream.block);
        }
        match &stream.reopen {
            Some(open) if !open.contains('\n') && !open.trim_start().starts_with('>') => {
                (self.from, self.opener) = (cut, Some(open.clone()));
            }
            _ if decided => (self.from, self.opener) = (self.block, None),
            _ => {}
        }
        StableStream {
            cut,
            reopen: stream.reopen,
            block: self.block,
        }
    }
}

/// Whether `text` ends inside a fence, by its unpaired fence markers.
pub(crate) fn open_fence(text: &str) -> bool {
    text.matches("```").count() % 2 == 1 || text.matches("~~~").count() % 2 == 1
}

pub(crate) fn line_start(source: &str, at: usize) -> usize {
    source
        .get(..at)
        .and_then(|head| head.rfind('\n'))
        .map_or(0, |nl| nl + 1)
}

fn last_block(source: &str) -> (usize, Option<OpenFence>) {
    let mut last_top = 0;
    let mut depth = 0usize;
    // Open lists: (start number, items seen, loose); open items: (start, shown number, list).
    let mut lists: Vec<(Option<u64>, u64, bool)> = Vec::new();
    let mut items: Vec<(usize, Option<u64>, usize)> = Vec::new();
    let mut quotes = 0usize;
    let mut fence: Option<OpenFence> = None;
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    last_top = range.start;
                    fence = None;
                }
                match tag {
                    Tag::List(start) => lists.push((start, 0, false)),
                    Tag::Item => {
                        let list = lists.len().saturating_sub(1);
                        let number = lists.last().and_then(|(start, seen, _)| {
                            start.map(|first| first.saturating_add(*seen))
                        });
                        if let Some(open) = lists.last_mut() {
                            open.1 += 1;
                        }
                        items.push((range.start, number, list));
                    }
                    Tag::Paragraph if !items.is_empty() => {
                        if let Some(open) = lists.last_mut() {
                            open.2 = true;
                        }
                    }
                    Tag::BlockQuote(_) => quotes += 1,
                    Tag::CodeBlock(CodeBlockKind::Fenced(_)) => {
                        let around = items
                            .iter()
                            .map(|&(start, number, list)| {
                                (start, number, lists.get(list).is_some_and(|open| open.2))
                            })
                            .collect();
                        fence = Some(OpenFence {
                            range,
                            contained: !items.is_empty() || quotes > 0,
                            around,
                        });
                    }
                    _ => {}
                }
                depth += 1;
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                match tag {
                    TagEnd::List(_) => {
                        lists.pop();
                    }
                    TagEnd::Item => {
                        items.pop();
                    }
                    TagEnd::BlockQuote(_) => quotes = quotes.saturating_sub(1),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (last_top, fence)
}

/// The end of the fence's last complete code line, and of its closing line when it has one.
/// The fence's range can leave its closing line out, so the scan runs past it.
fn fence_lines(
    source: &str,
    opener: &str,
    opener_end: usize,
    end: usize,
    contained: bool,
) -> (usize, Option<usize>) {
    fn run(line: &str, marker: char) -> (usize, &str) {
        let body = line.trim_start_matches([' ', '\t', '>']);
        (
            body.chars().take_while(|ch| *ch == marker).count(),
            body.trim(),
        )
    }
    // A closing line sits within three columns of its container: `\t```` at top level is code.
    let columns = |line: &str| {
        line.chars()
            .take_while(|ch| matches!(ch, ' ' | '\t' | '>'))
            .fold(0, |at, ch| {
                if ch == '\t' {
                    at + TAB_STOP - at % TAB_STOP
                } else {
                    at + 1
                }
            })
    };
    let marker = opener
        .trim_start_matches([' ', '\t', '>'])
        .chars()
        .next()
        .unwrap_or('`');
    let (open_len, _) = run(opener, marker);
    let indent = columns(opener);
    let mut last_line = opener_end;
    for line in source
        .get(opener_end..)
        .unwrap_or_default()
        .split_inclusive('\n')
    {
        if !line.ends_with('\n') {
            break;
        }
        let (length, trimmed) = run(line, marker);
        if length >= open_len
            && trimmed.chars().all(|ch| ch == marker)
            && columns(line) <= if contained { indent + 3 } else { 3 }
        {
            return (last_line, Some(last_line + line.len()));
        }
        if last_line >= end {
            break;
        }
        last_line += line.len();
    }
    (last_line, None)
}

/// The source a slice inside a nested fence renders under: a stub of each item around it,
/// then the opener.
fn fence_context(source: &str, around: &[(usize, Option<u64>, bool)], opener: &str) -> String {
    let mut context = String::new();
    for &(start, number, loose) in around {
        let end = if loose { "\n\n" } else { "\n" };
        context.push_str(&item_line(source, start, number, end));
    }
    context.push_str(opener.trim_end_matches(['\n', '\r']));
    context
}

/// The item at `start` as one stub line ending in `end`, at its own text column and shown by
/// the number the whole list gives it.
pub(crate) fn item_line(source: &str, start: usize, number: Option<u64>, end: &str) -> String {
    let prefix = stub_prefix(source, start);
    // The item's own marker keeps its text column; a stub before it sets the number it shows.
    let marker = item_marker(source.get(start..).unwrap_or_default()).unwrap_or("-");
    let written: Option<u64> = marker
        .get(..marker.len().saturating_sub(1))
        .and_then(|n| n.parse().ok());
    let mut line = String::new();
    if let Some(number) = number.filter(|number| Some(*number) != written && *number > 0) {
        let delimiter = marker.chars().last().unwrap_or('.');
        line.push_str(&format!("{prefix}{}{delimiter} x\n", number - 1));
    }
    line.push_str(&format!("{prefix}{marker} x{end}"));
    line
}

/// What stands before `start` on its line, quote marks kept and everything else blanked.
pub(crate) fn stub_prefix(source: &str, start: usize) -> String {
    source
        .get(line_start(source, start)..start)
        .unwrap_or_default()
        .chars()
        .map(|ch| {
            if ch == '>' || ch.is_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect()
}

struct Builder<'t> {
    theme: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    indent: String,
    /// Per open rail: the indent it replaced and the list depth it opened at.
    rails: Vec<(String, usize)>,
    list_stack: Vec<ListLevel>,
    pending_marker: Option<Span<'static>>,
    in_code_block: bool,
    /// Incident: pulldown-cmark splits CRLF code into `a`, `\nb`, `\n`, and a rail pushed per text
    /// event doubled it; code paints a whole line at a time.
    code_line: String,
    plain: bool,
    continued: bool,
    code_lang: &'t mut Option<crate::highlight::Lang>,
    link: Option<(String, String)>,
    table: Option<TableState>,
    item_gap: Option<bool>,
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

    /// Columns the list items opened since the innermost rail hang a row by. The hang is the
    /// width the markers occupy: a fixed two spaces left `10. ` and every nested glyph short.
    fn hang(&self) -> usize {
        let from = self.rails.last().map_or(0, |(_, depth)| *depth);
        self.list_stack
            .iter()
            .skip(from)
            .map(|level| level.hang)
            .sum()
    }

    /// Every row opened inside an item starts under its text, not only a wrap continuation.
    fn prefix(&self) -> String {
        format!("{}{}", self.indent, " ".repeat(self.hang()))
    }

    fn open_rail(&mut self, rail: &str) {
        let inner = format!("{}{rail}", self.prefix());
        let outer = std::mem::replace(&mut self.indent, inner);
        self.rails.push((outer, self.list_stack.len()));
    }

    fn close_rail(&mut self) {
        if let Some((outer, _)) = self.rails.pop() {
            self.indent = outer;
        }
    }

    fn flush_line(&mut self) {
        if !self.code_line.is_empty() {
            let line = std::mem::take(&mut self.code_line);
            self.code_row(&line);
        }
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let line = Line::from(mark_line(spans, self.theme));
        self.out
            .extend(wrap_line(&line, self.width, &self.prefix()));
    }

    // The marker is held, not written, so whichever line opens the item claims it. Written
    // eagerly, a loose list's wrapping paragraph flushed the marker alone above its text.
    fn line_prologue(&mut self) {
        if !self.spans.is_empty() {
            return;
        }
        let marker = self.pending_marker.take();
        let lead = if marker.is_some() {
            self.indent.clone()
        } else {
            self.prefix()
        };
        if !lead.is_empty() {
            let rail = Style::default().fg(self.theme.purple);
            self.spans.push(Span::styled(lead, rail));
        }
        self.spans.extend(marker);
    }

    fn settle_marker(&mut self) {
        if self.pending_marker.is_some() {
            self.line_prologue();
            self.flush_line();
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
        let pad = self
            .hang()
            .saturating_sub(self.list_stack.last().map_or(0, |level| level.hang));
        let marker = match self.list_stack.last_mut() {
            Some(ListLevel {
                next: Some(index), ..
            }) => {
                let marker = format!("{index}. ");
                *index = index.saturating_add(1);
                marker
            }
            // The assistant's own gutter is `•`, so a list never borrows it.
            _ => match depth {
                0 => "‣ ".to_owned(),
                1 => "◦ ".to_owned(),
                _ => "▪ ".to_owned(),
            },
        };
        if let Some(level) = self.list_stack.last_mut() {
            level.hang = UnicodeWidthStr::width(marker.as_str());
        }
        let style = self.style().fg(self.theme.accent);
        self.pending_marker = Some(Span::styled(format!("{}{marker}", " ".repeat(pad)), style));
    }

    fn push(&mut self, span: Span<'static>) {
        if let Some(table) = &mut self.table {
            if table.in_cell
                && let Some(cell) = table.current.last_mut()
            {
                cell.push_span(span);
            }
            return;
        }
        self.line_prologue();
        self.spans.push(span);
    }

    fn text(&mut self, text: &str) {
        if self.in_code_block {
            for raw in text.split_inclusive('\n') {
                match raw.strip_suffix('\n') {
                    Some(body) => {
                        self.code_line.push_str(body);
                        let line = std::mem::take(&mut self.code_line);
                        self.code_row(&line);
                    }
                    None => self.code_line.push_str(raw),
                }
            }
            return;
        }
        if let Some((_, label)) = &mut self.link {
            label.push_str(text);
        }
        // A row a `<br>` opened starts at its first word, as one after a markdown break does.
        let text = if self.spans.is_empty() && self.table.is_none() {
            text.trim_start()
        } else {
            text
        };
        if !text.is_empty() {
            self.push(Span::styled(inline_tabs(text), self.style()));
        }
    }

    fn code_row(&mut self, line: &str) {
        let body = expand_tabs(line);
        let base = self.theme.syntax_style(crate::highlight::Token::Plain);
        let rail = Span::styled(self.indent.clone(), self.theme.dim_style());
        let mut spans = vec![rail.clone()];
        match self.code_lang.as_mut() {
            Some(lang) => spans.extend(crate::highlight::spans(&body, lang, self.theme, base)),
            None => spans.push(Span::styled(body, base)),
        }
        self.out
            .extend(hard_wrap(&Line::from(spans), self.width, &rail));
    }

    fn hard_break(&mut self) {
        if let Some(table) = &mut self.table {
            if table.in_cell
                && let Some(cell) = table.current.last_mut()
            {
                cell.hard_break();
            }
        } else {
            self.flush_line();
        }
    }

    fn html(&mut self, html: &str) {
        for raw in html.split_inclusive('\n') {
            let body = raw.trim_end();
            if !body.is_empty() {
                self.push(Span::styled(expand_tabs(body), self.theme.dim_style()));
            }
            if raw.ends_with('\n') {
                self.flush_line();
            }
        }
    }
}

const TAB_STOP: usize = 4;

/// Incident: ratatui drops control characters, so a tab vanished with the indent it carried.
fn expand_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut column = 0;
    for ch in text.chars() {
        if ch == '\t' {
            let pad = TAB_STOP - column % TAB_STOP;
            out.extend(std::iter::repeat_n(' ', pad));
            column += pad;
        } else {
            out.push(ch);
            column += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    out
}

pub(crate) fn blank_before(source: &str, at: usize) -> bool {
    let line = source
        .get(..at)
        .and_then(|head| head.rfind('\n'))
        .unwrap_or(0);
    source
        .get(..line)
        .map(|head| head.rsplit('\n').next().unwrap_or(head))
        .is_some_and(|previous| line > 0 && previous.trim().is_empty())
}

/// Prose tabs take one fixed width: a tab stop depends on where the text began, and a streamed
/// slice begins elsewhere than the whole message.
pub(crate) fn inline_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_STOP))
}

fn is_break(html: &str) -> bool {
    html.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim_end()
        .eq_ignore_ascii_case("br")
}

/// Incident: pulldown-cmark splits text at `~` and `[`, so a path marked per event half-coloured.
fn mark_line(spans: Vec<Span<'static>>, theme: &Theme) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut run: Option<(Style, String)> = None;
    for span in spans {
        match &mut run {
            Some((style, text)) if *style == span.style => text.push_str(&span.content),
            _ => {
                if let Some((style, text)) = run.take() {
                    out.extend(marked_words(&text, style, theme));
                }
                if span.style.fg == Some(theme.text) {
                    run = Some((span.style, span.content.into_owned()));
                } else {
                    out.push(span);
                }
            }
        }
    }
    if let Some((style, text)) = run {
        out.extend(marked_words(&text, style, theme));
    }
    out
}

fn reduce_inline<'e>(b: &mut Builder, event: Event<'e>) -> Option<Event<'e>> {
    match event {
        Event::Start(Tag::Emphasis) => b.push_style(|s| s.add_modifier(Modifier::ITALIC)),
        Event::End(TagEnd::Emphasis) => b.pop_style(),
        // Weight, not hue: a bold paragraph in the heading's colour read as a heading.
        Event::Start(Tag::Strong) => b.push_style(|s| s.add_modifier(Modifier::BOLD)),
        Event::End(TagEnd::Strong) => b.pop_style(),
        Event::Start(Tag::Strikethrough) => {
            b.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT));
        }
        Event::End(TagEnd::Strikethrough) => b.pop_style(),
        Event::Start(Tag::Link { dest_url, .. }) => {
            b.link = Some((dest_url.to_string(), String::new()));
            b.push_style(|s| s.fg(b.theme.blue5).add_modifier(Modifier::UNDERLINED));
            b.text("");
        }
        Event::End(TagEnd::Link) => {
            b.pop_style();
            // The destination follows the label; a label alone drops the only
            // information a link carries, and a bare URL or address is its own label.
            if let Some((dest, label)) = b.link.take()
                && !dest.is_empty()
                && label != dest
                && dest.strip_prefix("mailto:") != Some(label.as_str())
            {
                b.push(Span::styled(format!(" ({dest})"), b.theme.dim_style()));
            }
        }
        Event::Code(code) => {
            let style = Style::default().fg(code_hue(b.theme, &code));
            b.push(Span::styled(inline_tabs(&code), style));
        }
        // Inline HTML is literal text a model wrote (`Vec<String>` unfenced), not markup to drop.
        Event::InlineHtml(html) if is_break(&html) => b.hard_break(),
        Event::InlineHtml(html) => b.text(&html),
        other => return Some(other),
    }
    None
}

fn reduce_table(b: &mut Builder, event: &Event) -> bool {
    match event {
        Event::Start(Tag::Table(alignments)) => {
            b.settle_marker();
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
                if let Some(cell) = table.current.last_mut() {
                    for line in &mut cell.lines {
                        line.spans = mark_line(std::mem::take(&mut line.spans), b.theme);
                    }
                }
            }
        }
        Event::End(TagEnd::Table) => {
            if let Some(table) = b.table.take() {
                let prefix = b.prefix();
                let available = b.width.saturating_sub(prefix.width());
                let rendered = crate::table::render(
                    table.header,
                    table.rows,
                    &table.alignments,
                    available,
                    b.theme,
                );
                for mut line in rendered {
                    line.spans.insert(0, Span::raw(prefix.clone()));
                    b.out.push(line);
                }
            }
        }
        _ => return false,
    }
    true
}

fn code_hue(theme: &Theme, code: &str) -> Color {
    let path_like = (code.contains('/') && !code.contains(' ')) || is_path(code);
    if path_like {
        crate::card::tool_hue(theme, "read")
    } else {
        theme.orange
    }
}

pub(crate) fn is_path(word: &str) -> bool {
    if word.contains(char::is_whitespace) {
        return false;
    }
    let rooted = ["./", "../", "~/"]
        .iter()
        .any(|root| word.starts_with(root))
        || (word.starts_with('/') && word.get(1..).is_some_and(|rest| rest.contains('/')));
    let mut bare = word;
    for _ in 0..2 {
        if let Some((head, number)) = bare.rsplit_once(':')
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
        {
            bare = head;
        }
    }
    let name = bare.rsplit('/').next().unwrap_or(bare);
    let extension = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        let product = ext == "js" && RUNTIMES.contains(&stem);
        (stem.contains(char::is_alphabetic) || ext.len() > 1)
            && !product
            && stem
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && KNOWN_EXTENSIONS.contains(&ext)
    });
    rooted || extension
}

const RUNTIMES: [&str; 6] = ["Node", "Next", "Nuxt", "React", "Vue", "Three"];

const KNOWN_EXTENSIONS: [&str; 44] = [
    "c", "cc", "cfg", "cpp", "css", "csv", "go", "h", "hpp", "html", "ini", "java", "jpg", "js",
    "json", "jsonl", "jsx", "kt", "lock", "log", "lua", "md", "mjs", "patch", "pdf", "php", "png",
    "py", "pyi", "rb", "rs", "scss", "sh", "sql", "svg", "swift", "toml", "ts", "tsx", "txt",
    "xml", "yaml", "yml", "zsh",
];

fn marked_words(text: &str, style: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let mut core = word
            .trim_start_matches(['(', '[', '"', '\''])
            .trim_end_matches(['.', ',', ';', ':', ']', '!', '?', '"', '\'']);
        while core.ends_with(')') && core.matches(')').count() > core.matches('(').count() {
            core = core
                .strip_suffix(')')
                .unwrap_or(core)
                .trim_end_matches(['.', ',', ';', ':']);
        }
        let hue = if core.starts_with("https://") || core.starts_with("http://") {
            Some(theme.blue5)
        } else if is_path(core) {
            Some(crate::card::tool_hue(theme, "read"))
        } else {
            None
        };
        let (Some(hue), Some(at)) = (hue, word.find(core).filter(|_| !core.is_empty())) else {
            plain.push_str(piece);
            continue;
        };
        plain.push_str(word.get(..at).unwrap_or_default());
        if !plain.is_empty() {
            out.push(Span::styled(std::mem::take(&mut plain), style));
        }
        out.push(Span::styled(core.to_owned(), style.fg(hue)));
        plain.push_str(
            piece
                .get(at.saturating_add(core.len())..)
                .unwrap_or_default(),
        );
    }
    if !plain.is_empty() || out.is_empty() {
        out.push(Span::styled(plain, style));
    }
    out
}

pub fn render(source: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    paint(source, width, theme, false, &mut None, false)
}

/// Uncoloured code for thoughts, which restyle every span: a 300-line fence spent 47 ms here.
pub fn render_plain(source: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    paint(source, width, theme, false, &mut None, true)
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
    paint(source, width, theme, continued, lang, false)
}

fn paint(
    source: &str,
    width: usize,
    theme: &Theme,
    continued: bool,
    lang: &mut Option<crate::highlight::Lang>,
    plain: bool,
) -> Vec<Line<'static>> {
    count(0, source.len());
    let width = width.max(4);
    let mut b = Builder {
        theme,
        width,
        out: Vec::new(),
        spans: Vec::new(),
        styles: vec![Style::default().fg(theme.text)],
        indent: String::new(),
        rails: Vec::new(),
        list_stack: Vec::new(),
        pending_marker: None,
        in_code_block: false,
        code_line: String::new(),
        plain,
        continued,
        code_lang: lang,
        link: None,
        table: None,
        item_gap: None,
    };
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        // Incident: list looseness, decided by text yet to arrive, re-spaced committed items; an item
        // opens on a blank row only when a blank line stands before it.
        let gap = match &event {
            Event::Start(Tag::Item) => Some(blank_before(source, range.start)),
            _ => None,
        };
        let opens_item = std::mem::replace(&mut b.item_gap, gap);
        if let (Some(false), Event::Start(Tag::Paragraph)) = (opens_item, &event) {
            continue;
        }
        let Some(event) = reduce_inline(&mut b, event) else {
            continue;
        };
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                b.blank();
                let (accent, cyan) = (b.theme.accent, b.theme.cyan);
                // The level has to be legible without the literal `#` marks
                // Yi drops.
                b.push_style(move |s| match level {
                    HeadingLevel::H1 => s.fg(accent).add_modifier(Modifier::BOLD),
                    HeadingLevel::H2 => s.fg(cyan).add_modifier(Modifier::BOLD),
                    HeadingLevel::H3 => s.add_modifier(Modifier::BOLD),
                    _ => s.add_modifier(Modifier::ITALIC),
                });
            }
            Event::End(TagEnd::Heading(level)) => {
                b.pop_style();
                b.flush_line();
                // A cell cannot grow, so the top level earns a rule beneath it instead.
                if level == HeadingLevel::H1 {
                    // Incident: at the full width, `> # Title` drew a rule two columns past it.
                    let prefix = b.prefix();
                    let length = b.width.saturating_sub(prefix.width()).min(40);
                    let rule: String = std::iter::repeat_n('━', length).collect();
                    b.out.push(Line::from(Span::styled(
                        format!("{prefix}{rule}"),
                        Style::default().fg(b.theme.accent),
                    )));
                }
                b.blank();
            }
            Event::Start(Tag::Paragraph) => b.blank(),
            Event::End(TagEnd::Paragraph) => b.flush_line(),
            Event::Start(Tag::BlockQuote(_)) => {
                b.settle_marker();
                b.blank();
                b.open_rail("▌ ");
                b.push_style(|s| s.add_modifier(Modifier::ITALIC));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                b.pop_style();
                b.flush_line();
                b.close_rail();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let continued = std::mem::take(&mut b.continued);
                b.settle_marker();
                b.blank();
                b.open_rail(CODE_RAIL_INDENT);
                // Fenced code gets a border rather than the author's backticks;
                // only a fence has an opening line a stream reopens.
                if let CodeBlockKind::Fenced(lang) = &kind {
                    if !continued {
                        *b.code_lang = if b.plain {
                            None
                        } else {
                            crate::highlight::lang_for(lang)
                        };
                        let head = format!("{}{lang}", b.indent);
                        b.out.push(Line::from(Span::styled(
                            head.trim_end().to_owned(),
                            b.theme.dim_style(),
                        )));
                    }
                } else if !continued {
                    *b.code_lang = None;
                }
                b.in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                b.flush_line();
                b.close_rail();
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
                b.settle_marker();
                b.flush_line();
            }
            event if reduce_table(&mut b, &event) => {}
            Event::Text(text) => b.text(&text),
            Event::SoftBreak => b.text(" "),
            Event::HardBreak => b.hard_break(),
            Event::Start(Tag::HtmlBlock) => b.blank(),
            Event::Html(html) => b.html(&html),
            Event::End(TagEnd::HtmlBlock) => b.flush_line(),
            Event::Rule => {
                b.blank();
                b.out.push(Line::from(Span::styled(
                    format!("{}———", b.prefix()),
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

/// Rows of a source that only grows, rendered past its last settled point alone: blocks
/// before the last one can no longer change, and an open fence settles line by line.
#[derive(Default)]
pub struct Settled {
    at: usize,
    reopen: Option<String>,
    lang: Option<crate::highlight::Lang>,
    shown: bool,
    opaque: bool,
}

impl Settled {
    /// The rows newly settled and the rows of the rest, which extend every earlier call's
    /// settled rows to `render(source)`. `None` once a link definition makes rows global.
    pub fn advance(
        &mut self,
        source: &str,
        width: usize,
        theme: &Theme,
    ) -> Option<(Vec<Line<'static>>, Vec<Line<'static>>)> {
        let rest = source.get(self.at..)?;
        self.opaque |= rest.lines().any(|line| {
            let body = line.trim_start_matches(' ');
            line.len() - body.len() <= 3 && body.starts_with('[') && body.contains("]:")
        });
        if self.opaque {
            return None;
        }
        let complete = rest.rfind('\n').map_or(0, |at| at + 1);
        let mut settled = Vec::new();
        if let Some((cut, reopen)) = self.settle_point(rest.get(..complete)?) {
            let mut lang = self.lang.take();
            settled = self.segment(rest.get(..cut)?, width, theme, &mut lang);
            (self.at, self.reopen, self.lang) = (self.at + cut, reopen, lang);
            self.shown |= !settled.is_empty();
        }
        let tail = source.get(self.at..)?;
        Some((
            settled,
            self.segment(tail, width, theme, &mut self.lang.clone()),
        ))
    }

    /// The last top-level block's start, or past the last complete line of its open fence.
    fn settle_point(&self, text: &str) -> Option<(usize, Option<String>)> {
        let open = self.reopen.as_ref().map(|line| format!("{line}\n"));
        let doc = format!("{}{text}", open.as_deref().unwrap_or_default());
        let (mut depth, mut last, mut code) = (0_usize, None, 0_usize);
        for (event, range) in Parser::new_ext(&doc, OPTIONS).into_offset_iter() {
            match &event {
                Event::Text(body) if depth == 1 => code += body.len(),
                Event::End(_) => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                _ => {
                    let line = doc.get(..range.start)?.rfind('\n').map_or(0, |at| at + 1);
                    let fenced = matches!(
                        event,
                        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))
                    );
                    (last, code) = (Some((line, fenced)), 0);
                }
            }
            if let Event::Start(_) = event {
                depth += 1;
            }
        }
        let (start, fenced) = last?;
        let opener = doc.get(start..)?.split_inclusive('\n').next()?;
        // A fence whose code runs to the end is still open, and each complete line is final.
        let open_fence = fenced && start + opener.len() + code == doc.len();
        let (cut, reopen) = match open_fence && opener.ends_with('\n') {
            true => (doc.len(), Some(opener.trim_end_matches('\n').to_owned())),
            false => (start, None),
        };
        let cut = cut.checked_sub(open.map_or(0, |line| line.len()))?;
        (cut > 0).then_some((cut, reopen))
    }

    /// A continued fence joins the rows above; any other block gets the blank row between.
    fn segment(
        &self,
        text: &str,
        width: usize,
        theme: &Theme,
        lang: &mut Option<crate::highlight::Lang>,
    ) -> Vec<Line<'static>> {
        let (mut text, mut reopen) = (text, self.reopen.as_deref());
        // A fence closing before any code leaves no row to set the next block off from.
        if let Some(open) = reopen {
            let doc = format!("{open}\n{text}");
            let mut events = Parser::new_ext(&doc, OPTIONS).into_offset_iter();
            if let (Some((Event::Start(_), block)), Some((Event::End(_), _))) =
                (events.next(), events.next())
            {
                let after = block.end.saturating_sub(open.len() + 1);
                (text, reopen) = (text.get(after..).unwrap_or_default(), None);
            }
        }
        let source = match reopen {
            Some(open) if !text.is_empty() => format!("{open}\n{text}"),
            _ => text.to_owned(),
        };
        let mut rows = render_stream(&source, width, theme, reopen.is_some(), lang);
        if self.shown && reopen.is_none() && !rows.is_empty() {
            rows.insert(0, Line::default());
        }
        rows
    }
}
