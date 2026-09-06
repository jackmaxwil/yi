use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::colors::{ColorTier, Theme, name_accent};
use crate::diffview::{self, DiffBudget};
use crate::markdown;
use crate::wrap::wrap_line;
use yi_types::subagent::ChildActivity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptMode {
    Normal,
    Thinking,
    Verbose,
}

/// Reasoning is the default view (U32 revised): `normal` collapses a thought to
/// a one-line count, which is only what a reader who asked for it should get.
impl Default for TranscriptMode {
    fn default() -> Self {
        Self::Thinking
    }
}

impl TranscriptMode {
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Thinking,
            Self::Thinking => Self::Verbose,
            Self::Verbose => Self::Normal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Thinking => "thinking",
            Self::Verbose => "verbose",
        }
    }
}

pub const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn spinner_frame(phase: usize) -> char {
    SPINNER_FRAMES
        .get(phase % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or('⠋')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    /// Held at the permission gate: the call itself is coloured, not only the
    /// prompt, so the row that is waiting says so.
    Awaiting,
    Done,
    Failed,
    Denied,
}

#[derive(Debug, Clone)]
pub struct ToolCell {
    pub name: String,
    /// The runtime's handle for this call: what a permission event names, and
    /// the only exact way to pair a result with the cell that started it.
    pub call_id: String,
    pub intent: Option<String>,
    pub status: ToolStatus,
    pub summary: String,
    pub digest: Option<String>,
    pub preview: Vec<String>,
    pub elapsed_ms: u64,
    pub calls: u32,
    /// The tool's own typed record — an edit's patch, a kernel cell's streams —
    /// merged from the call's arguments and its result.
    pub details: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct TaskCell {
    pub child_id: String,
    pub description: String,
    pub status: TaskStatus,
    pub last_tool: Option<String>,
    pub toolcalls: u32,
    pub tokens: u64,
    pub elapsed_ms: u64,
    pub error: Option<String>,
    // ponytail: the spawning cell's preview is observed, not plumbed — the `ipython` call
    // running when a child appears made it, and a child born outside one carries None.
    pub spawn: Option<String>,
    pub answer: Option<String>,
    pub activity: ChildActivity,
}

#[derive(Debug, Clone)]
pub enum Cell {
    User { text: String },
    Assistant { markdown: String },
    Thought { markdown: String },
    Tool(ToolCell),
    Explored(Vec<ToolCell>),
    Task(TaskCell),
    Advisory { source: String, text: String },
    Notice { text: String },
    Footer { text: String },
    Rule { text: String, accent_name: String },
    Divider,
}

pub const GUTTER: &str = "• ";
pub const CALLOUT_RAIL: &str = "▌";
const GUTTER_CONTINUATION: &str = "  ";
const THOUGHT_INDENT: &str = "  ";

/// Reasoning prose, dim and italic under a `∴` glyph. `header` is false past the
/// first slice: a thought streams a paragraph at a time, and a label each reads as many.
pub fn thought_lines(
    markdown: &str,
    width: usize,
    theme: &Theme,
    mode: TranscriptMode,
    header: bool,
) -> Vec<Line<'static>> {
    let style = theme.dim_style().add_modifier(Modifier::ITALIC);
    let glyph = Span::styled("  ∴", style.fg(theme.purple));
    if mode == TranscriptMode::Normal {
        let lines = markdown.lines().count();
        return vec![Line::from(vec![
            glyph,
            Span::styled(format!(" {lines} lines"), style),
        ])];
    }
    let mut out = Vec::new();
    if header {
        out.push(Line::default());
        out.push(Line::from(glyph));
    }
    // Rendered two columns narrow, matching the indent below: at full width every line that
    // filled it wrapped again and shed its last word onto a line of its own.
    for line in markdown::render(markdown, width.saturating_sub(THOUGHT_INDENT.len()), theme) {
        let text: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        out.extend(wrap_line(
            &Line::from(Span::styled(
                format!("{THOUGHT_INDENT}{}", text.trim_start()),
                style,
            )),
            width,
            THOUGHT_INDENT,
        ));
    }
    out
}

/// Assistant prose hangs off a dim `• ` on its first line and a two-column
/// gutter after it, so it is attributable without a box or a colour band.
pub fn gutter(lines: Vec<Line<'static>>, first: bool, theme: &Theme) -> Vec<Line<'static>> {
    let mut marked = first;
    lines
        .into_iter()
        .map(|line| {
            let content = !line.spans.iter().all(|s| s.content.trim().is_empty());
            // A callout carries its own rail; a dot beside it marks the same
            // block twice.
            let railed = line
                .spans
                .first()
                .is_some_and(|s| s.content.trim_start().starts_with(CALLOUT_RAIL));
            let prefix = if marked && content && !railed {
                marked = false;
                Span::styled(GUTTER.to_owned(), theme.dim_style())
            } else {
                marked = marked && !content;
                Span::raw(GUTTER_CONTINUATION.to_owned())
            };
            let mut spans = vec![prefix];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// Pad every row to the full width and paint the block style across it, so the
/// tint reads as one band instead of ragged per-line highlights.
fn tint(lines: Vec<Line<'static>>, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let style = theme.user_style();
    if style == Style::default() {
        return lines;
    }
    lines
        .into_iter()
        .map(|line| {
            let used: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let mut spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content, style.patch(s.style)))
                .collect();
            spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), style));
            Line::from(spans)
        })
        .collect()
}

/// The user turn carries a heavy left bar in the session accent over a panel
/// fill. The bar is what survives at 16 colors, where the tint degrades away.
const USER_BAR: &str = "┃";

fn bar(lines: Vec<Line<'static>>, theme: &Theme) -> Vec<Line<'static>> {
    let style = theme.user_style().fg(theme.accent);
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::styled(USER_BAR.to_owned(), style)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

fn glyph(tool: &str) -> char {
    match tool {
        "bash" => '$',
        "read" => '→',
        "edit" | "write" => '←',
        "grep" | "glob" | "find" => '✱',
        "fetch" | "web_search" => '%',
        "ipython" => '⊙',
        _ => '⚙',
    }
}

/// A row past three screen lines is generated, not written: two lines and its size.
fn capped(line: &Line<'static>, width: usize, indent: &str, theme: &Theme) -> Vec<Line<'static>> {
    let rows = wrap_line(line, width, indent);
    if rows.len() <= 3 {
        return rows;
    }
    let bytes: usize = line.spans.iter().map(|s| s.content.len()).sum();
    let mut out: Vec<Line<'static>> = rows.into_iter().take(2).collect();
    let size = format!("{indent}… {} KB", bytes.div_ceil(1024));
    out.push(Line::from(Span::styled(size, theme.dim_style())));
    out
}

pub(crate) fn count_label(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The hashline `[path#TAG]` header and the `NN:` row prefixes are anchors the
/// model edits against, not content the reader asked for.
fn strip_hashline(line: &str) -> &str {
    if line.starts_with('[') {
        return line;
    }
    numbered(line).map_or(line, |(_, text)| text)
}

/// `None` for a line with no number, which is how the hashline header and every
/// non-file tool fall through to a plain row.
fn numbered(line: &str) -> Option<(&str, &str)> {
    let (number, text) = line.split_once(':')?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some((number, text))
}

/// Split so consecutive hits in one file print the path once, not once per row.
fn grep_row(line: &str) -> Option<(&str, &str, &str)> {
    let (colon, _) = line.match_indices(':').find(|(colon, _)| {
        let rest = line.get(colon.saturating_add(1)..).unwrap_or_default();
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        digits > 0 && rest.get(digits..).is_some_and(|tail| tail.starts_with(':'))
    })?;
    let path = line.get(..colon)?;
    let rest = line.get(colon.saturating_add(1)..)?;
    let (number, text) = rest.split_once(':')?;
    Some((path, number, text))
}

pub(crate) fn elapsed_label(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 1000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

/// `412 lines`, `3 hits`: a digest that is a count and its noun, nothing else.
fn counted(digest: &str) -> Option<(&str, &str)> {
    let (count, noun) = digest.split_once(' ')?;
    let numeric = !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit());
    (numeric && !noun.is_empty() && !noun.contains(' ')).then_some((count, noun))
}

/// Read-only calls group under one bullet: a run of eight is one act of
/// looking, and eight rows of it crowds out the answer.
pub fn explore_verb(tool: &str) -> Option<&'static str> {
    match tool {
        "read" => Some("Read"),
        "grep" => Some("Search"),
        "glob" | "find" => Some("List"),
        _ => None,
    }
}

impl ToolCell {
    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        spinner_phase: usize,
    ) -> Vec<Line<'static>> {
        if self.name == "ipython" {
            return crate::pycell::lines(self, width, theme, mode, spinner_phase);
        }
        let expanded = mode == TranscriptMode::Verbose;
        let failed = matches!(self.status, ToolStatus::Failed | ToolStatus::Denied);
        let style = match self.status {
            ToolStatus::Awaiting => Style::default().fg(theme.warning),
            ToolStatus::Failed => Style::default().fg(theme.error),
            ToolStatus::Denied => theme.muted_style().add_modifier(Modifier::CROSSED_OUT),
            ToolStatus::Running | ToolStatus::Done => Style::default().fg(theme.text),
        };
        let mut spans = match self.status {
            ToolStatus::Running => vec![Span::styled(
                format!("{} ", spinner_frame(spinner_phase)),
                theme.accent_style(),
            )],
            ToolStatus::Awaiting => vec![Span::styled("△ ", style)],
            _ => Vec::new(),
        };
        spans.extend(self.summary_spans(theme, style));
        let (chips, digest) = self.chips(theme);
        let inner = crate::card::body_width(width);
        let mut body = Vec::new();
        // A failure's body is never mode-gated: a reader who cannot see why a
        // call failed cannot act on it, whatever mode the cell rendered under.
        if let Some(digest) = digest {
            let detail = if failed {
                Style::default().fg(theme.error)
            } else {
                theme.muted_style()
            };
            body.extend(capped(&Line::styled(digest, detail), inner, "  ", theme));
        }
        if let Some(patch) = self.patch() {
            let budget = if expanded {
                DiffBudget::FULL
            } else {
                DiffBudget::NORMAL
            };
            body.extend(diffview::render(patch, inner, theme, budget));
        } else if expanded {
            for line in self.body(theme) {
                body.extend(capped(&line, inner, "     ", theme));
            }
        }
        crate::card::card(Line::from(spans), &chips, body, width, theme, self.status)
    }

    /// The counts of the outcome as chips, and the digest that is prose rather than a count,
    /// which goes to the body instead.
    fn chips(&self, theme: &Theme) -> (Vec<crate::card::Chip>, Option<String>) {
        use crate::card::Chip;
        let mut chips = Vec::new();
        if self.calls > 1 {
            chips.push(Chip::new(
                format!("×{}", self.calls),
                "",
                theme.muted_style(),
            ));
        }
        if let Some(intent) = &self.intent
            && self.status == ToolStatus::Running
        {
            chips.push(Chip::new(intent.clone(), "", theme.muted_style()));
        }
        let mut prose = None;
        if let Some(digest) = &self.digest {
            match counted(digest) {
                Some((count, noun)) => {
                    chips.push(Chip::new(count, noun, Style::default().fg(theme.text)));
                }
                None => prose = Some(digest.clone()),
            }
        }
        let count = |key: &str| self.details.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (added, removed) = (count("added"), count("removed"));
        if added > 0 || removed > 0 {
            chips.push(Chip::new(
                format!("+{added}"),
                "",
                Style::default().fg(theme.success),
            ));
            chips.push(Chip::new(
                format!("−{removed}"),
                "",
                Style::default().fg(theme.error),
            ));
        }
        if self.status != ToolStatus::Running
            && let Some(code) = self.details.get("exitCode").and_then(Value::as_i64)
        {
            chips.push(Chip::exit(code, theme));
        }
        chips.extend(Chip::elapsed(self.elapsed_ms, theme));
        (chips, prose)
    }

    fn patch(&self) -> Option<&str> {
        self.details.get("patch")?.as_str()
    }

    /// A shell command is code, so it renders as code rather than as a tool
    /// argument: a dim `$` then the command itself, highlighted.
    fn summary_spans(&self, theme: &Theme, style: Style) -> Vec<Span<'static>> {
        let command = self.summary.strip_prefix("$ ");
        let Some((mut lang, command)) = crate::highlight::lang_for("bash").zip(command) else {
            let name = Style::default()
                .fg(crate::card::tool_hue(theme, &self.name))
                .add_modifier(Modifier::BOLD);
            return match self.summary.split_once(self.name.as_str()) {
                Some((glyph, rest)) => {
                    let mut spans = vec![Span::styled(format!("{glyph}{}", self.name), name)];
                    // A path is read by its basename; the directories are context, so dim.
                    match rest.rsplit_once('/') {
                        Some((dir, base)) if !rest.contains(' ') && !base.is_empty() => {
                            spans.push(Span::styled(format!("{dir}/"), theme.dim_style()));
                            spans.push(Span::styled(base.to_owned(), style));
                        }
                        _ => spans.push(Span::styled(rest.to_owned(), style)),
                    }
                    spans
                }
                None => vec![Span::styled(self.summary.clone(), style)],
            };
        };
        let mut spans = vec![Span::styled("$ ".to_owned(), theme.dim_style())];
        spans.extend(crate::highlight::spans(command, &mut lang, theme, style));
        spans
    }

    /// Typed, not a raw dump: a read hangs off a line-number gutter and a search
    /// groups hits under each path rather than repeating it ten times.
    fn body(&self, theme: &Theme) -> Vec<Line<'static>> {
        let dim = theme.dim_style();
        let text = Style::default().fg(theme.text);
        let mut out = Vec::new();
        let mut last_path: Option<String> = None;
        let searchy = matches!(self.name.as_str(), "grep" | "glob" | "find");
        let anchored = matches!(self.name.as_str(), "read" | "edit");
        // A patch routes to `diffview`, which colours from its own header; this is the plain
        // numbered dump a read prints, its path in the summary since `details` lacks it.
        let subject = anchored
            .then(|| self.summary.split_once(self.name.as_str()))
            .flatten()
            .map(|(_, rest)| rest.trim())
            .filter(|subject| !subject.is_empty());
        let mut lang = subject.and_then(crate::highlight::lang_for);
        let mut expect = 1_u64;
        for raw in &self.preview {
            let row = if searchy { grep_row(raw) } else { None };
            if let Some((path, number, body)) = row {
                if last_path.as_deref() != Some(path) {
                    out.push(Line::from(Span::styled(path.to_owned(), dim)));
                    last_path = Some(path.to_owned());
                }
                out.push(Line::from(vec![
                    Span::styled(format!("{number:>4} "), dim),
                    Span::styled(body.to_owned(), text),
                ]));
                continue;
            }
            match numbered(raw) {
                Some((number, body)) => {
                    let mut spans = vec![Span::styled(format!("{number:>4} "), dim)];
                    // The parse describes only the rows fed to it from line 1, so a preview
                    // opening mid-file leaves it describing text the reader never saw.
                    if number.parse::<u64>().ok() != Some(expect) {
                        lang = None;
                    }
                    match lang.as_mut() {
                        Some(lang) => {
                            spans.extend(crate::highlight::spans(body, lang, theme, text));
                        }
                        None => spans.push(Span::styled(body.to_owned(), text)),
                    }
                    // An over-wide row arrives clipped, so its tail, and any quote or block
                    // comment closing in it, never reached the parse the next rows resume from.
                    if body.ends_with('\u{2026}') {
                        lang = None;
                    }
                    expect = expect.saturating_add(1);
                    out.push(Line::from(spans));
                }
                // The hashline header repeats the path the head line already
                // names, and the verb line is the digest above it.
                None if anchored && !raw.starts_with('…') => {}
                None => out.push(Line::from(Span::styled(raw.to_owned(), dim))),
            }
        }
        out
    }

    pub fn summary_of(name: &str, argument: &str) -> String {
        // A shell call names itself: `$ cargo test` reads, `$ bash cargo test`
        // says the word "bash" where the command should be.
        if name == "bash" && !argument.is_empty() {
            return format!("$ {argument}");
        }
        format!("{} {} {}", glyph(name), name, argument)
            .trim_end()
            .to_owned()
    }

    /// Yi showed no result lines, so a finished call left no trace of its
    /// outcome unless the reader had switched to verbose before it ran.
    pub fn digest_of(name: &str, text: &str, failed: bool) -> Option<String> {
        let first = || {
            text.lines()
                .map(str::trim_end)
                .find(|line| !line.trim().is_empty())
        };
        if failed {
            return first().map(strip_hashline).map(str::to_owned);
        }
        let numbered = || {
            text.lines()
                .filter(|line| !line.starts_with('[') && strip_hashline(line) != *line)
                .count()
        };
        let entries = || {
            text.lines()
                .filter(|line| line.ends_with('/') || line.ends_with(" B"))
                .count()
        };
        let digest = match name {
            "read" if numbered() == 0 && entries() > 0 => count_label(entries(), "entry"),
            "read" => count_label(numbered(), "line"),
            // `[path#TAG]` then `updated; first change at line N` — the verb
            // and the anchor the model just earned, in the tool's own words.
            "edit" => text.lines().nth(1)?.trim().to_owned(),
            "grep" => count_label(
                text.lines().filter(|l| grep_row(l).is_some()).count(),
                "hit",
            ),
            "glob" => count_label(
                text.lines().filter(|line| !line.starts_with('[')).count(),
                "file",
            ),
            _ => first()?.to_owned(),
        };
        let digest = digest.trim().to_owned();
        (!digest.is_empty()).then_some(digest)
    }
}

impl TaskCell {
    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        spinner_phase: usize,
    ) -> Vec<Line<'static>> {
        let activity = match self.activity {
            ChildActivity::Waiting => "waiting",
            ChildActivity::Writing => "writing",
            ChildActivity::Executing => "executing",
        };
        let pulse = crate::motion::pulse_frame(crate::motion::elapsed_of(spinner_phase));
        let (glyph, state, tone) = match self.status {
            TaskStatus::Running => (pulse, activity, theme.purple),
            TaskStatus::Done => ('↳', "done", theme.purple),
            TaskStatus::Failed => ('✗', "failed", theme.error),
        };
        let (name, hash) = humanize(&self.description);
        let hash = if hash.is_empty() {
            hash
        } else {
            format!(" · {hash}")
        };
        let calls = count_label(
            usize::try_from(self.toolcalls).unwrap_or(usize::MAX),
            "tool call",
        );
        let elapsed = elapsed_label(self.elapsed_ms);
        let title = format!("{glyph} {name}{hash} · {state} {elapsed} · {calls}");
        let mut body = Vec::new();
        if self.status == TaskStatus::Running {
            let tool = self.last_tool.as_deref().unwrap_or("starting");
            let row = format!(
                "⚙ {tool} · {} tokens",
                crate::status::fmt_tokens(self.tokens)
            );
            body.push(Line::from(Span::styled(row, theme.muted_style())));
        }
        if let (TaskStatus::Failed, Some(error)) = (&self.status, &self.error) {
            let error: String = error.chars().take(80).collect();
            body.push(Line::from(Span::styled(
                error,
                Style::default().fg(theme.error),
            )));
        }
        let answer = self.answer.as_deref().filter(|a| !a.trim().is_empty());
        let rows: Vec<Line<'static>> = answer
            .map(|a| markdown::render(a, width.saturating_sub(6), theme))
            .unwrap_or_default()
            .into_iter()
            .skip_while(|line| line.spans.is_empty())
            .collect();
        let total = rows.len();
        let running = self.status == TaskStatus::Running;
        let (skip, take) = match (running, mode) {
            (true, _) => (total.saturating_sub(4), 4),
            (false, TranscriptMode::Verbose) => (0, total),
            (false, _) => (0, 3),
        };
        let len = total.saturating_sub(skip).min(take);
        body.extend(
            rows.into_iter()
                .skip(skip)
                .take(take)
                .enumerate()
                .map(|(index, line)| {
                    let age = if running {
                        len.saturating_sub(index).saturating_sub(1)
                    } else {
                        0
                    };
                    fade(line, age, len, theme)
                }),
        );
        if total > skip.saturating_add(take) {
            let more = format!("… {} more lines", total.saturating_sub(take));
            body.push(Line::from(Span::styled(more, theme.dim_style())));
        }
        boxed(&title, body, width, Style::default().fg(tone))
    }
}

/// A child's prose is retained state it controls; the card keeps the last 16 KB.
pub fn tail_bounded(text: String) -> String {
    let cut = text.len().saturating_sub(16 * 1024);
    match text
        .char_indices()
        .map(|(index, _)| index)
        .find(|i| *i >= cut)
    {
        Some(start) if cut > 0 => text.get(start..).unwrap_or_default().to_owned(),
        _ => text,
    }
}

fn humanize(name: &str) -> (String, String) {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let hashed = clean
        .rsplit_once('-')
        .filter(|(_, tail)| tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_hexdigit()));
    let (head, hash) = hashed.unwrap_or((clean.as_str(), ""));
    let mut chars = head.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    (
        format!("{first}{}", chars.as_str()).replace('-', " "),
        hash.to_owned(),
    )
}

fn fade(line: Line<'static>, age: usize, total: usize, theme: &Theme) -> Line<'static> {
    let style = match (age, theme.tier) {
        (0, _) => return line,
        (_, ColorTier::TrueColor) => {
            let alpha = age.saturating_mul(255).checked_div(total).unwrap_or(0);
            let alpha = u16::try_from(alpha).unwrap_or(u16::MAX);
            Style::default().fg(crate::motion::blend(theme.muted, theme.dim, alpha))
        }
        _ => theme.dim_style(),
    };
    let spans = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, s.style.patch(style)));
    Line::from(spans.collect::<Vec<_>>())
}

/// A rounded frame from `Line`s alone; under eight inner columns the frame is dropped.
fn boxed(title: &str, body: Vec<Line<'static>>, width: usize, style: Style) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(6);
    if inner < 8 {
        let head = Line::from(Span::styled(format!("  {title}"), style));
        return [Line::default(), head]
            .into_iter()
            .chain(body)
            .chain([Line::default()])
            .collect();
    }
    let mut used = 0_usize;
    let title: String = title
        .chars()
        .take_while(|c| {
            used = used.saturating_add(UnicodeWidthChar::width(*c).unwrap_or(0));
            used <= width.saturating_sub(8)
        })
        .collect();
    let fill = width
        .saturating_sub(7)
        .saturating_sub(UnicodeWidthStr::width(title.as_str()));
    let top = format!("  ╭─ {title} {}╮", "─".repeat(fill));
    let bottom = format!("  ╰{}╯", "─".repeat(width.saturating_sub(4)));
    let mut out = vec![Line::default(), Line::from(Span::styled(top, style))];
    for row in body.iter().flat_map(|line| wrap_line(line, inner, "")) {
        let used: usize = row
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let mut spans = vec![Span::styled("  │ ", style)];
        spans.extend(row.spans);
        spans.push(Span::styled(
            format!("{} │", " ".repeat(inner.saturating_sub(used))),
            style,
        ));
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled(bottom, style)));
    out.push(Line::default());
    out
}

impl Cell {
    pub fn lines(
        &self,
        width: usize,
        theme: &Theme,
        mode: TranscriptMode,
        spinner_phase: usize,
    ) -> Vec<Line<'static>> {
        match self {
            Cell::User { text } => {
                let mut out = vec![Line::default()];
                let width = width.saturating_sub(1);
                for raw in text.lines() {
                    out.extend(wrap_line(
                        &Line::from(Span::styled(
                            format!("   {raw}"),
                            Style::default().fg(theme.text),
                        )),
                        width,
                        "   ",
                    ));
                }
                out.push(Line::default());
                bar(tint(out, width, theme), theme)
            }
            Cell::Assistant { markdown } => {
                let mut out = vec![Line::default()];
                out.extend(gutter(
                    markdown::render(markdown, width.saturating_sub(GUTTER.len()), theme),
                    true,
                    theme,
                ));
                out
            }
            Cell::Thought { markdown } => thought_lines(markdown, width, theme, mode, true),
            Cell::Tool(tool) => tool.lines(width, theme, mode, spinner_phase),
            Cell::Explored(rows) => explored_lines(rows, width, theme),
            Cell::Task(task) => task.lines(width, theme, mode, spinner_phase),
            Cell::Advisory { source, text } => {
                // The advisor speaks over the agent's own output, so it takes the callout
                // rail and blank air rather than a dim aside that reads as more prose.
                let clean = strip_tags(text);
                let rail = Style::default().fg(theme.warning);
                let body = wrap_line(
                    &Line::from(vec![
                        Span::styled(format!("⚑ {source} "), rail.add_modifier(Modifier::BOLD)),
                        Span::styled(clean, theme.muted_style()),
                    ]),
                    width.saturating_sub(4),
                    "",
                );
                let mut out = vec![Line::default()];
                out.extend(body.into_iter().map(|line| {
                    let mut spans = vec![Span::styled(format!("  {CALLOUT_RAIL} "), rail)];
                    spans.extend(line.spans);
                    Line::from(spans)
                }));
                out.push(Line::default());
                out
            }
            Cell::Notice { text } => {
                let style = Style::default().fg(theme.warning);
                let rows = format!("  ⚑ {}", text.replace('\n', "\n    "));
                rows.lines()
                    .flat_map(|row| {
                        let line = Line::from(Span::styled(row.to_owned(), style));
                        wrap_line(&line, width, "    ")
                    })
                    .collect()
            }
            Cell::Footer { text } => {
                let line = Line::from(Span::styled(format!("  ↳ {text}"), theme.dim_style()));
                wrap_line(&line, width, "    ")
            }
            Cell::Divider => {
                let fill: String = std::iter::repeat_n('─', width.saturating_sub(4)).collect();
                vec![
                    Line::default(),
                    Line::from(Span::styled(format!("  {fill}"), theme.dim_style())),
                ]
            }
            Cell::Rule { text, accent_name } => {
                let accent = name_accent(accent_name);
                let label = format!("── {text} ");
                let fill_width = width.saturating_sub(label.chars().count()).min(40);
                let fill: String = std::iter::repeat_n('─', fill_width).collect();
                vec![Line::from(Span::styled(
                    format!("{label}{fill}"),
                    Style::default().fg(accent),
                ))]
            }
        }
    }
}

const EXPLORED_CAP: usize = 32;
/// Fixed, not measured: a width read off one run would leave two adjacent
/// Explored blocks with their subjects in different columns.
const VERB_WIDTH: usize = 6;

/// A run of read-only calls, one row each under a single bullet: the verb in
/// accent, its subject, and the digest the call earned.
fn explored_lines(rows: &[ToolCell], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let head = Line::from(vec![
        Span::styled(
            "✱ Explored",
            Style::default()
                .fg(crate::card::tool_hue(theme, "read"))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ×{}", rows.len()), theme.muted_style()),
    ]);
    let inner = crate::card::body_width(width);
    let mut out = Vec::new();
    for row in rows.iter().take(EXPLORED_CAP) {
        let verb = explore_verb(&row.name).unwrap_or("Ran");
        // The summary opens with the tool's own glyph and name, which the verb
        // column now says; what is left is the subject.
        let subject = row
            .summary
            .split_once(&row.name)
            .map(|(_, rest)| rest.trim())
            .unwrap_or(row.summary.as_str())
            .to_owned();
        let mut spans = vec![
            Span::styled(
                format!("{verb:<VERB_WIDTH$} "),
                Style::default().fg(theme.accent),
            ),
            Span::styled(subject, Style::default().fg(theme.text)),
        ];
        if let Some(digest) = &row.digest {
            spans.push(Span::styled(format!("  {digest}"), theme.dim_style()));
        }
        out.extend(wrap_line(&Line::from(spans), inner, "        "));
    }
    if let Some(extra) = rows.len().checked_sub(EXPLORED_CAP).filter(|n| *n > 0) {
        out.push(Line::from(Span::styled(
            format!("… {extra} more"),
            theme.dim_style(),
        )));
    }
    crate::card::card(head, &[], out, width, theme, ToolStatus::Done)
}

/// Advisories arrive as `<advisory …>text</advisory>` markup; the tags are
/// model-facing structure, not user content.
pub fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
