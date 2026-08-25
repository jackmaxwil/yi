use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::{Theme, name_accent};
use crate::markdown;
use crate::wrap::wrap_line;

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
    pub preview: Vec<String>,
    pub elapsed_ms: u64,
    pub calls: u32,
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
}

fn glyph(tool: &str) -> char {
    match tool {
        "bash" => '$',
        "read" => '→',
        "edit" | "write" => '←',
        "grep" | "glob" | "find" => '✱',
        "fetch" | "web_search" => '%',
        _ => '⚙',
    }
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
        expanded: bool,
        spinner_phase: usize,
    ) -> Vec<Line<'static>> {
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
        if expanded {
            for raw in &self.preview {
                let text = format!("    {raw}");
                lines.extend(wrap_line(
                    &Line::from(Span::styled(text, theme.dim_style())),
                    width,
                    "    ",
                ));
            }
        }
        lines
    }

    pub fn summary_of(name: &str, argument: &str) -> String {
        format!("{} {} {}", glyph(name), name, argument)
            .trim_end()
            .to_owned()
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
        expanded: bool,
        spinner_phase: usize,
    ) -> Vec<Line<'static>> {
        match self {
            Cell::User { text } => {
                let mut out = vec![Line::default()];
                for raw in text.lines() {
                    out.extend(wrap_line(
                        &Line::from(vec![
                            Span::styled("  › ", Style::default().fg(theme.accent)),
                            Span::styled(raw.to_owned(), Style::default().fg(theme.text)),
                        ]),
                        width,
                        "    ",
                    ));
                }
                out
            }
            Cell::Assistant { markdown } => {
                let mut out = vec![Line::default()];
                out.extend(markdown::render(markdown, width, theme));
                out
            }
            Cell::Thought { markdown } => {
                let rendered = markdown::render(markdown, width, theme);
                let mut out = Vec::new();
                for line in rendered {
                    let text: String = line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>();
                    out.extend(wrap_line(
                        &Line::from(Span::styled(format!("  {text}"), theme.dim_style())),
                        width,
                        "  ",
                    ));
                }
                out
            }
            Cell::Tool(tool) => tool.lines(width, theme, expanded, spinner_phase),
            Cell::Task(task) => task.lines(width, theme, spinner_phase),
            Cell::Advisory { source, text } => wrap_line(
                &Line::from(Span::styled(
                    format!("  ⋯ {source}: {text}"),
                    theme.dim_style().add_modifier(Modifier::ITALIC),
                )),
                width,
                "    ",
            ),
            Cell::Notice { text } => wrap_line(
                &Line::from(Span::styled(
                    format!("  ⚑ {text}"),
                    Style::default().fg(theme.warning),
                )),
                width,
                "    ",
            ),
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
