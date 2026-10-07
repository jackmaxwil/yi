use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::cell::spinner_frame;
use crate::colors::{Theme, name_accent};

#[derive(Debug, Clone, Default)]
pub struct StatusInput {
    pub model: String,
    /// The router a namespaced id went through, shown as `model via provider`.
    pub provider: Option<String>,
    pub thinking: Option<String>,
    pub mode: Option<String>,
    pub cwd: String,
    /// `repo ⎇ lane N`: shown instead of `path@branch` when the session is on a lane.
    pub lane: Option<String>,
    pub branch: Option<String>,
    pub landing: Option<String>,
    pub cost: Option<String>,
    /// The session's `N% cached`, on a route that reads a cache.
    pub cache: Option<String>,
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

/// Tokens and cache hits read so far, kept once per turn (the footer) and once per session.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TokenTally {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cached: u64,
}

impl TokenTally {
    pub(crate) fn record(&mut self, usage: &yi_types::message::Usage) {
        let read = usage
            .input
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        self.input = bump(self.input, read);
        self.output = bump(self.output, usage.output);
        self.cached = bump(self.cached, usage.cache_read);
    }

    /// `N% cached` once input was read, where a read was expected or one happened.
    pub(crate) fn cache_label(self, expected: bool) -> Option<String> {
        (self.input > 0 && (expected || self.cached > 0))
            .then(|| format!("{}% cached", self.cached * 100 / self.input))
    }
}

/// Dollars spent by a turn or a session; `lower_bound` once a reply came back without usage.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Money {
    cost: f64,
    lower_bound: bool,
}

impl Money {
    pub(crate) fn record(&mut self, usage: &yi_types::message::Usage) {
        self.cost += usage.cost.total.as_f64().unwrap_or(0.0);
        self.lower_bound |= usage.unknown;
    }

    /// `None` until something was spent or went unreported.
    pub(crate) fn label(self) -> Option<String> {
        (self.cost > 0.0 || self.lower_bound)
            .then(|| yi_types::message::fmt_cost(self.cost, self.lower_bound))
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

/// A landing older than two poll periods says so; a fresh one is silent about its age.
const LANDING_FRESH: std::time::Duration = std::time::Duration::from_secs(120);

fn age_suffix(age: Option<std::time::Duration>) -> String {
    match age {
        Some(age) if age >= LANDING_FRESH => {
            let secs = age.as_secs();
            if secs < 3_600 {
                format!(" · {} min ago", secs / 60)
            } else {
                format!(" · {} h ago", secs / 3_600)
            }
        }
        _ => String::new(),
    }
}

/// The status-row form: one glyph per gate job, how far `main` moved, and how old the
/// reading is once the poll may have stopped.
pub fn landing_segment(
    landing: &yi_types::lane::Landing,
    age: Option<std::time::Duration>,
) -> Option<String> {
    use yi_types::lane::Landing;
    match landing {
        Landing::Unlanded => None,
        Landing::Pushed { .. } => Some("pushed".to_owned()),
        Landing::Open { pr, jobs, behind } => {
            let glyphs: String = jobs.iter().map(|job| job.state.glyph()).collect();
            let behind = match behind {
                0 => String::new(),
                n => format!(" · main +{n}"),
            };
            Some(format!("PR {pr} {glyphs}{behind}{}", age_suffix(age)))
        }
        Landing::Merged { pr } => Some(format!("PR {pr} merged")),
    }
}

/// What the row still shows: the input with the cuts so far applied.
struct Fit {
    input: StatusInput,
    path_max: usize,
    name_max: usize,
    place: bool,
}

/// The drop order, applied first to last until the row fits. The order never changes, only
/// how many apply, and each cut takes a whole field; the model is never cut.
const CUTS: [fn(&mut Fit); 12] = [
    |fit| fit.input.cache = None,
    |fit| fit.input.landing = None,
    |fit| fit.input.subagents = 0,
    |fit| fit.name_max = NAME_FLOOR,
    |fit| fit.path_max = PATH_FLOOR,
    |fit| fit.input.mode = None,
    |fit| fit.input.provider = None,
    |fit| fit.input.session_name.clear(),
    |fit| fit.input.thinking = None,
    |fit| fit.input.context_window = 0,
    |fit| fit.input.cost = None,
    |fit| fit.place = false,
];

/// Cells kept empty between the groups, so they never read as one run.
const GAP_MIN: usize = 4;

fn seg_style(input: &StatusInput, theme: &Theme) -> Style {
    if input.focused_child.is_some() {
        theme.dim_style()
    } else {
        theme.muted_style()
    }
}

/// What and where: the model and how it runs, then the place and its landing.
fn left_spans(fit: &Fit, theme: &Theme) -> Vec<Span<'static>> {
    let input = &fit.input;
    let seg_style = seg_style(input, theme);
    let mut spans = vec![Span::styled(" ", seg_style)];
    if let Some(child) = &input.focused_child {
        spans.push(Span::styled(
            format!("👻 {child} "),
            Style::default().fg(theme.warning),
        ));
    }
    let model_style = if input.focused_child.is_some() {
        seg_style
    } else {
        Style::default().fg(theme.text)
    };
    spans.push(Span::styled(format!(" {}", input.model), model_style));
    if let Some(provider) = &input.provider {
        spans.push(Span::styled(format!(" via {provider}"), seg_style));
    }
    spans.push(Span::styled(" ", model_style));
    if let Some(level) = &input.thinking {
        spans.push(Span::styled("· ", theme.dim_style()));
        spans.push(Span::styled(
            format!("◉ {level} "),
            effort_style(level, theme),
        ));
    }
    let place = fit.place.then(|| match &input.lane {
        Some(lane) => lane.clone(),
        None => {
            let mut path = shrink_left(&input.cwd, fit.path_max);
            if let Some(branch) = &input.branch {
                path.push_str(&format!("@{}", shrink_left(branch, fit.path_max)));
            }
            path
        }
    });
    let mode = input.mode.as_ref().map(|mode| format!("◉ {mode}"));
    for seg in mode.into_iter().chain(place).chain(input.landing.clone()) {
        spans.push(Span::styled(" · ", theme.dim_style()));
        spans.push(Span::styled(format!(" {seg} "), seg_style));
    }
    spans
}

/// The meters, then the session's tile and name.
fn right_spans(fit: &Fit, theme: &Theme) -> Vec<Span<'static>> {
    let input = &fit.input;
    let seg_style = seg_style(input, theme);
    let mut segs: Vec<String> = Vec::new();
    if input.subagents > 0 {
        segs.push(format!("👥 {}", input.subagents));
    }
    segs.extend(input.cost.clone());
    segs.extend(input.cache.clone());
    if input.context_window > 0 {
        segs.push(format!(
            "{} / {}",
            fmt_exact(input.context_used),
            fmt_tokens(input.context_window)
        ));
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for seg in segs {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme.dim_style()));
        }
        spans.push(Span::styled(format!(" {seg} "), seg_style));
    }
    if !input.session_name.is_empty() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme.dim_style()));
        }
        let display: String = if input.session_name.chars().count() > 16 {
            input.session_name.chars().take(8).collect()
        } else {
            input.session_name.clone()
        };
        let style = if input.focused_child.is_some() {
            seg_style
        } else {
            Style::default()
                .fg(name_accent(&input.session_name))
                .add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(
            crate::colors::name_tile(&input.session_name),
            crate::colors::tile_style(&input.session_name),
        ));
        spans.push(Span::styled(
            format!(" {} ", shrink_middle(&display, fit.name_max)),
            style,
        ));
    }
    spans
}

/// `text` less at least `over` cells, whole characters only, ending in `…`. Measures the
/// string, not the characters: `⚠️` is 1 + 0 cells apart and 2 together.
fn clip_cells(text: &str, over: usize) -> String {
    let keep = text.width().saturating_sub(over + 1);
    let cells = crate::wrap::flatten_spans(&[Span::raw(text)]);
    let kept = crate::wrap::take_cells(&cells, keep);
    let mut out: String = kept.iter().map(|cell| cell.ch).collect();
    out.push('…');
    out
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Identity anchored left and meters right, so a number gaining a digit never shifts the
/// model; [`CUTS`] apply until the rendered spans fit with one column free.
pub fn render(input: &StatusInput, width: usize, theme: &Theme) -> Line<'static> {
    let mut fit = Fit {
        input: input.clone(),
        path_max: 24,
        name_max: 24,
        place: true,
    };
    let mut cuts = CUTS.iter();
    let (mut spans, right) = loop {
        let (left, right) = (left_spans(&fit, theme), right_spans(&fit, theme));
        let gap = if right.is_empty() { 0 } else { GAP_MIN };
        match cuts.next() {
            Some(cut) if spans_width(&left) + gap + spans_width(&right) >= width => cut(&mut fit),
            _ => break (left, right),
        }
    };
    // Every cut spent and still too wide: the model loses its tail at a cell boundary.
    let over = (spans_width(&spans) + spans_width(&right) + 1).saturating_sub(width);
    if over > 0 {
        fit.input.model = clip_cells(&fit.input.model, over);
        spans = left_spans(&fit, theme);
    }
    let gap = width.saturating_sub(spans_width(&spans) + spans_width(&right) + 1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
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
