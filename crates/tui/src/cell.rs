use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::colors::{Theme, name_accent};
use crate::diffview::{self, DiffBudget};
use crate::markdown;
use crate::wrap::wrap_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptMode {
    Normal,
    Thinking,
    Verbose,
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
    Done,
    Failed,
    Denied,
}

#[derive(Debug, Clone)]
pub struct ToolCell {
    pub name: String,
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

impl Default for ToolCell {
    fn default() -> Self {
        Self {
            name: String::new(),
            intent: None,
            status: ToolStatus::Running,
            summary: String::new(),
            digest: None,
            preview: Vec::new(),
            elapsed_ms: 0,
            calls: 1,
            details: Value::Null,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct TaskCell {
    pub agent: String,
    pub child_id: String,
    pub description: String,
    pub status: TaskStatus,
    pub last_tool: Option<String>,
    pub toolcalls: u32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Cell {
    User { text: String },
    Assistant { markdown: String },
    Thought { markdown: String },
    Tool(ToolCell),
    Task(TaskCell),
    Advisory { source: String, text: String },
    Notice { text: String },
    Rule { text: String, accent_name: String },
    Divider,
}

pub const GUTTER: &str = "• ";
pub const CALLOUT_RAIL: &str = "▌";
const GUTTER_CONTINUATION: &str = "  ";

/// codex `history_cell/messages.rs:530`: assistant prose hangs off a dim `• `
/// on its first line and a two-column gutter after it, so a block of prose is
/// attributable at a glance without a box or a color band.
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

/// opencode `routes/session/index.tsx:1398-1420`: the user turn carries a
/// heavy left bar in the session accent over a panel fill. The bar is what
/// survives at 16 colors, where the tint degrades to nothing.
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

/// grep prints `path:N:text` for a hit and `path-N-text` for a context row, so
/// only a hit carries a `:N:` run. A path holding its own `:N:` would overcount
/// by one; a context row losing the distinction entirely would not.
fn is_grep_hit(line: &str) -> bool {
    line.match_indices(':').any(|(colon, _)| {
        let rest = line.get(colon.saturating_add(1)..).unwrap_or_default();
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        digits > 0 && rest.get(digits..).is_some_and(|tail| tail.starts_with(':'))
    })
}

fn count_label(count: usize, noun: &str) -> String {
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
    match line.split_once(':') {
        Some((number, text))
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            text
        }
        _ => line,
    }
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

fn elapsed_label(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 1000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

fn status_glyph(status: ToolStatus, spinner_phase: usize) -> char {
    match status {
        ToolStatus::Running => spinner_frame(spinner_phase),
        ToolStatus::Done => '✓',
        ToolStatus::Failed | ToolStatus::Denied => '✗',
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
        let style = match self.status {
            ToolStatus::Running => Style::default().fg(theme.text),
            ToolStatus::Done => theme.muted_style(),
            ToolStatus::Failed => Style::default().fg(theme.error),
            ToolStatus::Denied => theme.muted_style().add_modifier(Modifier::CROSSED_OUT),
        };
        let mut head = format!(
            "  {} {}",
            status_glyph(self.status, spinner_phase),
            self.summary
        );
        if self.calls > 1 {
            head.push_str(&format!(" ×{}", self.calls));
        }
        if let Some(intent) = &self.intent
            && self.status == ToolStatus::Running
        {
            head.push_str(&format!(" · {intent}"));
        }
        if self.elapsed_ms > 0 {
            head.push_str(&format!(" · {}", elapsed_label(self.elapsed_ms)));
        }
        let mut lines = wrap_line(&Line::from(Span::styled(head, style)), width, "    ");
        // A failure's body is never mode-gated: a reader who cannot see why a
        // call failed cannot act on it, whatever mode the cell rendered under.
        let failed = matches!(self.status, ToolStatus::Failed | ToolStatus::Denied);
        if let Some(digest) = &self.digest {
            let detail = if failed {
                Style::default().fg(theme.error)
            } else {
                theme.dim_style()
            };
            let mut spans = vec![Span::styled(format!("    └ {digest}"), detail)];
            spans.extend(self.stats_spans(theme));
            lines.extend(wrap_line(&Line::from(spans), width, "      "));
        }
        if let Some(patch) = self.patch() {
            let budget = if expanded {
                DiffBudget::FULL
            } else {
                DiffBudget::NORMAL
            };
            lines.extend(diffview::render(patch, width, theme, budget));
            return lines;
        }
        if expanded {
            for line in self.body(theme) {
                lines.extend(wrap_line(&line, width, "        "));
            }
        }
        lines
    }

    fn patch(&self) -> Option<&str> {
        self.details.get("patch")?.as_str()
    }

    /// `+12 -3` in the diff's own colours, on the line that already names the
    /// target — the counts a reader wants before deciding to read the body.
    fn stats_spans(&self, theme: &Theme) -> Vec<Span<'static>> {
        let count = |key: &str| self.details.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (added, removed) = (count("added"), count("removed"));
        if added == 0 && removed == 0 {
            return Vec::new();
        }
        vec![
            Span::styled(format!(" +{added}"), Style::default().fg(theme.success)),
            Span::styled(format!(" -{removed}"), Style::default().fg(theme.error)),
        ]
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
        for raw in &self.preview {
            let row = if searchy { grep_row(raw) } else { None };
            if let Some((path, number, body)) = row {
                if last_path.as_deref() != Some(path) {
                    out.push(Line::from(Span::styled(format!("      {path}"), dim)));
                    last_path = Some(path.to_owned());
                }
                out.push(Line::from(vec![
                    Span::styled(format!("      {number:>4} "), dim),
                    Span::styled(body.to_owned(), text),
                ]));
                continue;
            }
            match numbered(raw) {
                Some((number, body)) => out.push(Line::from(vec![
                    Span::styled(format!("      {number:>4} "), dim),
                    Span::styled(body.to_owned(), text),
                ])),
                // The hashline header repeats the path the head line already
                // names, and the verb line is the digest above it.
                None if anchored => {}
                None => out.push(Line::from(Span::styled(format!("      {raw}"), dim))),
            }
        }
        out
    }

    pub fn summary_of(name: &str, argument: &str) -> String {
        format!("{} {} {}", glyph(name), name, argument)
            .trim_end()
            .to_owned()
    }

    /// Yi showed no result lines at all, so a finished call left no trace of its
    /// outcome unless the reader had switched to verbose before it ran (codex
    /// commits five under `  └ `, OMP four).
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
        let digest = match name {
            "read" => count_label(numbered(), "line"),
            // `[path#TAG]` then `updated; first change at line N` — the verb
            // and the anchor the model just earned, in the tool's own words.
            "edit" => text.lines().nth(1)?.trim().to_owned(),
            "grep" => count_label(text.lines().filter(|line| is_grep_hit(line)).count(), "hit"),
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
    pub fn lines(&self, width: usize, theme: &Theme, spinner_phase: usize) -> Vec<Line<'static>> {
        let accent = name_accent(&self.agent);
        let (glyph, style) = match self.status {
            TaskStatus::Running => (spinner_frame(spinner_phase), Style::default().fg(accent)),
            TaskStatus::Done => ('✓', Style::default().fg(theme.success)),
            TaskStatus::Failed => ('✗', Style::default().fg(theme.error)),
        };
        let head = format!("  {glyph} {} Task — {}", self.agent, self.description);
        let detail = match (&self.status, &self.error, &self.last_tool) {
            (TaskStatus::Failed, Some(error), _) => {
                let mut error = error.clone();
                error.truncate(80);
                format!("    ↳ {error}")
            }
            (TaskStatus::Running, _, Some(tool)) => format!("    ↳ {tool}"),
            (TaskStatus::Running, _, None) => format!("    ↳ {} toolcalls", self.toolcalls),
            _ => format!(
                "    ↳ {} toolcalls · {}",
                self.toolcalls,
                elapsed_label(self.elapsed_ms)
            ),
        };
        let detail_style = if self.status == TaskStatus::Failed {
            Style::default().fg(theme.error)
        } else {
            theme.muted_style()
        };
        let mut lines = vec![Line::default()];
        lines.extend(wrap_line(
            &Line::from(Span::styled(head, style)),
            width,
            "    ",
        ));
        lines.extend(wrap_line(
            &Line::from(Span::styled(detail, detail_style)),
            width,
            "      ",
        ));
        lines.push(Line::default());
        lines
    }
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
                let width = width.saturating_sub(USER_BAR.len());
                for (index, raw) in text.lines().enumerate() {
                    // The bar carries the block; the caret marks only where it
                    // starts (codex `messages.rs:265`).
                    let marker = if index == 0 { " › " } else { "   " };
                    out.extend(wrap_line(
                        &Line::from(vec![
                            Span::styled(marker, Style::default().fg(theme.accent)),
                            Span::styled(raw.to_owned(), Style::default().fg(theme.text)),
                        ]),
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
            Cell::Thought { markdown } => {
                if mode == TranscriptMode::Normal {
                    let lines = markdown.lines().count();
                    return vec![Line::from(Span::styled(
                        format!("  ∴ thinking · {lines} lines"),
                        theme.dim_style().add_modifier(Modifier::ITALIC),
                    ))];
                }
                let rendered = markdown::render(markdown, width, theme);
                let mut out = vec![
                    Line::default(),
                    Line::from(Span::styled(
                        "  ∴ thinking".to_owned(),
                        theme.dim_style().add_modifier(Modifier::ITALIC),
                    )),
                ];
                for line in rendered {
                    let text: String = line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>();
                    out.extend(wrap_line(
                        &Line::from(Span::styled(
                            format!("  {}", text.trim_start()),
                            theme.dim_style().add_modifier(Modifier::ITALIC),
                        )),
                        width,
                        "  ",
                    ));
                }
                out
            }
            Cell::Tool(tool) => tool.lines(width, theme, mode, spinner_phase),
            Cell::Task(task) => task.lines(width, theme, spinner_phase),
            Cell::Advisory { source, text } => {
                // The advisor speaks over the agent's own output, so it takes
                // the callout rail and blank air rather than a dim aside that
                // reads as one more line of prose.
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
            Cell::Notice { text } => wrap_line(
                &Line::from(Span::styled(
                    format!("  ⚑ {text}"),
                    Style::default().fg(theme.warning),
                )),
                width,
                "    ",
            ),
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
