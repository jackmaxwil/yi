use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::cell::spinner_frame;
use crate::colors::{Theme, name_accent};

#[derive(Debug, Clone, Default)]
pub struct StatusInput {
    pub model: String,
    pub thinking: Option<String>,
    pub mode: Option<String>,
    pub cwd: String,
    pub branch: Option<String>,
    pub cost: Option<String>,
    pub session_name: String,
    pub subagents: usize,
    pub context_used: u64,
    pub context_window: u64,
    pub focused_child: Option<String>,
}

const NAME_FLOOR: usize = 8;
const PATH_FLOOR: usize = 8;

/// Paths truncate from the left — the tail (project dir) is the signal.
fn shrink_left(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - max.saturating_sub(1)).collect();
    let tail = tail
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or(tail);
    format!("…{tail}")
}

fn shrink_middle(text: &str, max: usize) -> String {
    if text.chars().count() <= max || max < 5 {
        return text.chars().take(max).collect();
    }
    let half = max / 2;
    let head: String = text.chars().take(half.saturating_sub(1)).collect();
    let tail: String = {
        let count = text.chars().count();
        text.chars()
            .skip(count.saturating_sub(max - half))
            .collect()
    };
    format!("{head}…{tail}")
}

/// A signed usage delta added onto a running total; a negative or overflowing delta is 0.
fn bump(base: u64, delta: i64) -> u64 {
    base.saturating_add(u64::try_from(delta).unwrap_or(0))
}

/// Tokens and cache hits accumulated so far this turn.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TurnTokens {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cached: u64,
}

impl TurnTokens {
    pub(crate) fn record(&mut self, usage: &yi_types::message::Usage) {
        let read = usage
            .input
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        self.input = bump(self.input, read);
        self.output = bump(self.output, usage.output);
        self.cached = bump(self.cached, usage.cache_read);
    }
}

pub(crate) fn fmt_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let m = tokens as f64 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as u64)
        } else {
            format!("{m:.1}M")
        }
    } else if tokens >= 1000 {
        format!("{}K", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

fn left_segments(input: &StatusInput, path_max: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut model = input.model.clone();
    if let Some(thinking) = &input.thinking {
        model.push_str(&format!(" · ◉ {thinking}"));
    }
    segments.push(model);
    segments.extend(input.mode.as_ref().map(|mode| format!("◉ {mode}")));
    let mut path = shrink_left(&input.cwd, path_max);
    if let Some(branch) = &input.branch {
        path.push_str(&format!("@{}", shrink_left(branch, path_max)));
    }
    segments.push(path);
    segments.extend(input.cost.clone());
    if input.context_window > 0 {
        segments.push(format!(
            "{} / {}",
            fmt_exact(input.context_used),
            fmt_tokens(input.context_window)
        ));
    }
    segments
}

fn fmt_exact(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn effort_style(level: &str, theme: &Theme) -> Style {
    match level {
        "off" => theme.dim_style(),
        "minimal" | "low" => theme.muted_style(),
        "medium" => Style::default().fg(theme.cyan),
        "high" => Style::default().fg(theme.warning),
        _ => Style::default().fg(theme.error),
    }
}

pub fn git_branch(cwd: &str) -> Option<String> {
    let mut dir = std::path::PathBuf::from(cwd);
    let git = loop {
        let candidate = dir.join(".git");
        if candidate.exists() {
            break candidate;
        }
        if !dir.pop() {
            return None;
        }
    };
    let git_dir = if git.is_file() {
        let pointer = std::fs::read_to_string(&git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir: ")?;
        let path = std::path::PathBuf::from(target);
        if path.is_absolute() {
            path
        } else {
            git.parent()?.join(path)
        }
    } else {
        git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(name) => Some(name.to_owned()),
        None => Some(head.chars().take(8).collect()),
    }
}

fn right_segments(input: &StatusInput, name_max: usize) -> Vec<String> {
    let mut segments = Vec::new();
    if input.subagents > 0 {
        segments.push(format!("👥 {}", input.subagents));
    }
    if !input.session_name.is_empty() {
        let display = if input.session_name.chars().count() > 16 {
            input.session_name.chars().take(8).collect()
        } else {
            input.session_name.clone()
        };
        segments.push(shrink_middle(&display, name_max));
    }
    segments
}

/// Overflow cascade: shrink the session name, pop right segments, shrink the path and the
/// branch under one budget, then drop left segments right to left with the model last.
pub fn render(input: &StatusInput, width: usize, theme: &Theme) -> Line<'static> {
    let accent = name_accent(&input.session_name);
    let accent_style = Style::default().fg(accent);
    let mut path_max = 24_usize;
    let mut name_max = 24_usize;
    let mut left = left_segments(input, path_max);
    let mut right = right_segments(input, name_max);
    let measure = |left: &[String], right: &[String]| -> usize {
        let left_w: usize = left.iter().map(|s| s.width() + 3).sum();
        let right_w: usize = right.iter().map(|s| s.width() + 3).sum();
        left_w + right_w + 2
    };
    while measure(&left, &right) > width && name_max > NAME_FLOOR {
        name_max = name_max.saturating_sub(4).max(NAME_FLOOR);
        right = right_segments(input, name_max);
    }
    while measure(&left, &right) > width && right.len() > 1 {
        right.pop();
    }
    while measure(&left, &right) > width && path_max > PATH_FLOOR {
        path_max = path_max.saturating_sub(8).max(PATH_FLOOR);
        left = left_segments(input, path_max);
    }
    let path_index = if input.mode.is_some() { 2 } else { 1 };
    while measure(&left, &right) > width && left.len() > 1 {
        let drop = (1..left.len())
            .rev()
            .find(|&i| i != path_index)
            .unwrap_or(path_index);
        left.remove(drop);
    }

    let dimmed = input.focused_child.is_some();
    let seg_style = if dimmed {
        theme.dim_style()
    } else {
        theme.muted_style()
    };
    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ", seg_style)];
    if let Some(child) = &input.focused_child {
        spans.push(Span::styled(
            format!("👻 {child} "),
            Style::default().fg(theme.warning),
        ));
    }
    for (i, seg) in left.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme.dim_style()));
        }
        let style = if i == 0 && !dimmed {
            Style::default().fg(theme.text)
        } else {
            seg_style
        };
        if i == 0
            && let Some((model, level)) = seg.split_once(" · ◉ ")
        {
            spans.push(Span::styled(format!(" {model} "), style));
            spans.push(Span::styled("· ", theme.dim_style()));
            spans.push(Span::styled(
                format!("◉ {level} "),
                effort_style(level, theme),
            ));
            continue;
        }
        spans.push(Span::styled(format!(" {seg} "), style));
    }
    let right_spans: Vec<Span<'static>> = {
        let mut out: Vec<Span<'static>> = Vec::new();
        for (i, seg) in right.iter().enumerate() {
            if i > 0 {
                out.push(Span::styled(" · ", theme.dim_style()));
            }
            let last = i == right.len() - 1;
            if last && !input.session_name.is_empty() {
                out.push(Span::styled(
                    crate::colors::name_tile(&input.session_name),
                    crate::colors::tile_style(&input.session_name),
                ));
            }
            let style = if last && !dimmed {
                accent_style.add_modifier(Modifier::BOLD)
            } else {
                seg_style
            };
            out.push(Span::styled(format!(" {seg} "), style));
        }
        out
    };
    let used: usize = spans
        .iter()
        .map(|s| s.content.as_ref().width())
        .sum::<usize>()
        + right_spans
            .iter()
            .map(|s| s.content.as_ref().width())
            .sum::<usize>();
    let gap = width.saturating_sub(used + 1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right_spans);
    Line::from(spans)
}

/// Spinner + the current tool's `i` intent + interrupt hint.
pub fn working_line(
    intent: Option<&str>,
    spinner_phase: usize,
    esc_armed: bool,
    theme: &Theme,
) -> Line<'static> {
    let glyph = spinner_frame(spinner_phase);
    let text = intent.unwrap_or("Working…");
    let hint = if esc_armed {
        "esc again to interrupt"
    } else {
        "[esc] interrupt"
    };
    let elapsed = crate::motion::elapsed_of(spinner_phase);
    let mut spans = vec![Span::styled(
        format!(" {glyph} "),
        Style::default().fg(theme.accent),
    )];
    // The narration is the one row that is always in flight, so it carries the
    // sweep rather than a second animated glyph beside the spinner.
    spans.extend(crate::motion::shimmer(text, elapsed, theme));
    spans.push(Span::styled(format!("  {hint}"), theme.dim_style()));
    Line::from(spans)
}
