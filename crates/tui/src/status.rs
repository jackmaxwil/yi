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
    pub threshold_pct: Option<u8>,
    pub focused_child: Option<String>,
}

const NAME_FLOOR: usize = 8;
const PATH_FLOOR: usize = 8;

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

fn left_segments(input: &StatusInput, path_max: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut model = format!("⬢ {}", input.model);
    if let Some(thinking) = &input.thinking {
        model.push_str(&format!(" · ◉ {thinking}"));
    }
    segments.push(model);
    if let Some(mode) = &input.mode {
        segments.push(format!("◉ {mode}"));
    }
    let mut path = shrink_middle(&input.cwd, path_max);
    if let Some(branch) = &input.branch {
        path.push_str(&format!("@{branch}"));
    }
    segments.push(path);
    if let Some(cost) = &input.cost {
        segments.push(cost.clone());
    }
    segments
}

fn right_segments(input: &StatusInput, name_max: usize) -> Vec<String> {
    let mut segments = Vec::new();
    if input.subagents > 0 {
        segments.push(format!("👥 {}", input.subagents));
    }
    if !input.session_name.is_empty() {
        segments.push(shrink_middle(&input.session_name, name_max));
    }
    segments
}

/// The context gauge (OMP `#buildContextGaugeFill`, adapted): the gap between
/// the groups is a `─` bar — used portion in the session accent, a heavier
/// tick at the auto-compact threshold, the percent label near the fill head
/// and the window label right-anchored, both skipped when the gap is narrow.
fn gauge(
    input: &StatusInput,
    gap: usize,
    theme: &Theme,
    accent_style: Style,
) -> Vec<Span<'static>> {
    if gap == 0 {
        return Vec::new();
    }
    if gap < 8 || input.context_window == 0 {
        let fill: String = std::iter::repeat_n('─', gap).collect();
        return vec![Span::styled(fill, theme.dim_style())];
    }
    let pct = (input.context_used * 100 / input.context_window).min(120);
    let percent_label = format!("{pct}%");
    let window_label = format!("{}K", input.context_window / 1000);
    let bar_len = gap;
    let mut cells: Vec<(char, bool, bool)> = (0..bar_len).map(|_| ('─', false, false)).collect();
    let filled = ((pct.min(100) as usize) * bar_len) / 100;
    let filled = if input.context_used > 0 {
        filled.max(1)
    } else {
        filled
    };
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.1 = i < filled;
    }
    if let Some(threshold) = input.threshold_pct {
        let index = ((threshold as usize) * bar_len / 100).min(bar_len.saturating_sub(1));
        if let Some(cell) = cells.get_mut(index) {
            cell.0 = '┃';
        }
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let overflow = pct > 100;
    let label_room = percent_label.len() + window_label.len() + 4;
    let with_labels = bar_len >= label_room + 4;
    let percent_at = if with_labels {
        filled.min(bar_len.saturating_sub(label_room))
    } else {
        bar_len
    };
    let mut i = 0;
    while i < bar_len {
        if with_labels && i == percent_at {
            let style = if overflow {
                Style::default().fg(theme.error)
            } else {
                accent_style
            };
            spans.push(Span::styled(percent_label.clone(), style));
            i += percent_label.len();
            continue;
        }
        if with_labels && i == bar_len.saturating_sub(window_label.len() + 1) {
            spans.push(Span::styled(window_label.clone(), theme.muted_style()));
            i += window_label.len();
            continue;
        }
        let (ch, lit, _) = cells.get(i).copied().unwrap_or(('─', false, false));
        let style = if ch == '┃' {
            theme.muted_style()
        } else if lit {
            accent_style
        } else {
            theme.dim_style()
        };
        spans.push(Span::styled(ch.to_string(), style));
        i += 1;
    }
    spans
}

/// U16: one status row above the composer. Overflow runs OMP's named
/// truncation cascade (`status-line/component.ts:1878-1943`): shrink the
/// session name to a floor, pop right segments, shrink the path to a floor,
/// then drop left segments from the end — skipping the path, so cwd
/// survives longest (the naive version collapsed the bar to just the model).
pub fn render(input: &StatusInput, width: usize, theme: &Theme) -> Line<'static> {
    let accent = name_accent(&input.session_name);
    let accent_style = Style::default().fg(accent);
    let mut path_max = 40_usize;
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
        let drop = (0..left.len()).rev().find(|&i| i != path_index);
        match drop {
            Some(i) if left.len() > 1 => {
                left.remove(i);
            }
            _ => break,
        }
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
        spans.push(Span::styled(format!(" {seg} "), style));
    }
    let right_spans: Vec<Span<'static>> = {
        let mut out: Vec<Span<'static>> = Vec::new();
        for (i, seg) in right.iter().enumerate() {
            if i > 0 {
                out.push(Span::styled(" · ", theme.dim_style()));
            }
            let style = if i == right.len() - 1 && !dimmed {
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
    spans.extend(gauge(input, gap, theme, accent_style));
    spans.extend(right_spans);
    Line::from(spans)
}

/// The working line under the composer: spinner + the current tool's `i`
/// intent (OMP: the model narrates what it thinks it is doing) + interrupt
/// hint, `esc again to interrupt` after the first press.
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
    Line::from(vec![
        Span::styled(format!(" {glyph} "), Style::default().fg(theme.accent)),
        Span::styled(text.to_owned(), Style::default().fg(theme.text)),
        Span::styled(format!("  {hint}"), theme.dim_style()),
    ])
}
