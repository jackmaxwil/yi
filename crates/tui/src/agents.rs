use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::cell::{Cell, TaskStatus, ToolStatus};
use crate::colors::{Theme, name_accent};
use crate::keymap::SingleKey;
use crate::popup::{BottomView, PopupResult};

/// In-row confirm window: long enough to be deliberate, short enough that a
/// stray key does not stay armed while the reader looks away.
const CONFIRM: Duration = Duration::from_secs(2);
const BAR_CELLS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub id: String,
    pub name: String,
    pub state: AgentState,
    pub tokens: u64,
    pub toolcalls: u64,
    /// The kernel cell that spawned it, shown once above the children it made.
    pub spawn: Option<String>,
}

pub struct AgentsPopup {
    rows: Vec<AgentRow>,
    context_window: u64,
    selected: usize,
    /// Which row is one keystroke from being stopped, and since when.
    armed: Option<(usize, Instant)>,
    /// The child the user chose to interrupt; the event loop drains it.
    pub stop: Option<String>,
}

fn glyph(state: AgentState) -> char {
    match state {
        AgentState::Running => '◆',
        AgentState::Done => '✓',
        AgentState::Failed => '✗',
    }
}

fn tokens_label(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{}.{}M", tokens / 1_000_000, (tokens % 1_000_000) / 100_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

impl AgentsPopup {
    pub fn new(rows: Vec<AgentRow>, context_window: u64) -> Self {
        Self {
            rows,
            context_window,
            selected: 0,
            armed: None,
            stop: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The window lapses on its own, so an armed row that the reader walked
    /// away from disarms rather than waiting to be pressed.
    fn armed_row(&self) -> Option<usize> {
        self.armed
            .filter(|(_, at)| at.elapsed() < CONFIRM)
            .map(|(index, _)| index)
    }

    /// The `/context` bar: the root's own share of its window, in ten cells,
    /// warning once the window is most of the way gone.
    fn bar(&self, tokens: u64, theme: &Theme) -> Vec<Span<'static>> {
        if self.context_window == 0 {
            return Vec::new();
        }
        let share = tokens.saturating_mul(BAR_CELLS) / self.context_window.max(1);
        let filled = usize::try_from(share.min(BAR_CELLS)).unwrap_or(0);
        let percent = tokens.saturating_mul(100) / self.context_window.max(1);
        // U16's rule for the context gauge: over the window is an error, not a
        // warning, and the bar clamps while the number keeps telling the truth.
        let style = match percent {
            0..=79 => Style::default().fg(theme.accent),
            80..=100 => Style::default().fg(theme.warning),
            _ => Style::default().fg(theme.error),
        };
        let empty = usize::try_from(BAR_CELLS)
            .unwrap_or(0)
            .saturating_sub(filled);
        vec![
            Span::styled("▓".repeat(filled), style),
            Span::styled("░".repeat(empty), theme.dim_style()),
            Span::styled(format!(" {percent}%"), style),
        ]
    }
}

impl BottomView for AgentsPopup {
    fn lines(&self, _width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(Span::styled(
            "  agent                      tokens   calls   context".to_owned(),
            theme.muted_style().add_modifier(Modifier::BOLD),
        ))];
        let armed = self.armed_row();
        let last = self.rows.len().saturating_sub(1);
        let mut group: Option<&str> = None;
        for (index, row) in self.rows.iter().enumerate() {
            // The cell that made a family is shown once above it, not
            // repeated on every child.
            if let Some(spawn) = row.spawn.as_deref()
                && group != Some(spawn)
            {
                group = Some(spawn);
                out.push(Line::from(Span::styled(
                    format!(" ⊙ {spawn}"),
                    theme.dim_style(),
                )));
            }
            let connector = if index == last {
                " └─ "
            } else {
                " ├─ "
            };
            let selected = index == self.selected;
            let name_style = if selected {
                Style::default()
                    .fg(name_accent(&row.name))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(name_accent(&row.name))
            };
            if armed == Some(index) {
                out.push(Line::from(vec![
                    Span::styled(connector.to_owned(), theme.dim_style()),
                    Span::styled(
                        format!("{} x again to stop", row.name),
                        Style::default().fg(theme.error),
                    ),
                ]));
                continue;
            }
            let mut spans = vec![
                Span::styled(connector.to_owned(), theme.dim_style()),
                Span::styled(format!("{} ", glyph(row.state)), name_style),
                Span::styled(format!("{:<20}", row.name), name_style),
                Span::styled(
                    format!("{:>8} ", tokens_label(row.tokens)),
                    theme.muted_style(),
                ),
                Span::styled(format!("{:>6}   ", row.toolcalls), theme.muted_style()),
            ];
            spans.extend(self.bar(row.tokens, theme));
            out.push(Line::from(spans));
        }
        let total: u64 = self.rows.iter().map(|row| row.tokens).sum();
        out.push(Line::from(Span::styled(
            format!(
                " total {} over {} agent{}   ↑↓ select · x stop · esc close",
                tokens_label(total),
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" }
            ),
            theme.dim_style(),
        )));
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        use crate::keymap::KeyCodeValue;
        match key.code {
            KeyCodeValue::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.armed = None;
            }
            KeyCodeValue::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(self.rows.len().saturating_sub(1));
                self.armed = None;
            }
            KeyCodeValue::Char('x') => match self.armed_row() {
                Some(index) if index == self.selected => {
                    self.stop = self.rows.get(index).map(|row| row.id.clone());
                    self.armed = None;
                    return PopupResult::Close;
                }
                _ => self.armed = Some((self.selected, Instant::now())),
            },
            KeyCodeValue::Esc | KeyCodeValue::Enter => return PopupResult::Close,
            _ => {}
        }
        PopupResult::Open
    }
}

/// The view's own half of the `App`: how the family becomes rows, and what a
/// stop does.
impl crate::app::App {
    /// Every child's own token use, so the column sums.
    pub fn open_agents(&mut self) {
        let rows = self
            .task_order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .map(|state| AgentRow {
                id: state.cell.child_id.clone(),
                name: state.cell.description.clone(),
                state: match state.cell.status {
                    TaskStatus::Running => AgentState::Running,
                    TaskStatus::Done => AgentState::Done,
                    TaskStatus::Failed => AgentState::Failed,
                },
                tokens: state.cell.tokens,
                toolcalls: u64::from(state.cell.toolcalls),
                spawn: state.cell.spawn.clone(),
            })
            .collect::<Vec<_>>();
        let popup = AgentsPopup::new(rows, self.options.context_window);
        if popup.is_empty() {
            self.commit_cell(&Cell::Notice {
                text: "no subagents in this session".to_owned(),
            });
            return;
        }
        self.bottom = Some(crate::app::Bottom::Agents(popup));
        self.scheduler.request();
    }

    /// The kernel cell a child was born under, if one is running.
    pub(crate) fn spawning_cell(&self) -> Option<String> {
        let tool = self
            .live_tools
            .iter()
            .rev()
            .find(|tool| tool.name == "ipython" && tool.status != ToolStatus::Done)?;
        let code = tool.details.get("code")?.as_str()?;
        Some(crate::pycell::preview(code)).filter(|line| !line.is_empty())
    }

    /// That cell's call id: the one cell a finished card waits on before it commits.
    pub(crate) fn spawning_call(&self) -> Option<String> {
        self.live_tools
            .iter()
            .rev()
            .find(|tool| tool.name == "ipython" && tool.status != ToolStatus::Done)
            .map(|tool| tool.call_id.clone())
    }

    /// The host's `interrupt` ends a run by aborting the child's own session, which the App
    /// already holds; the record it keeps besides that is the host's either way.
    pub fn stop_child(&mut self, child_id: &str) {
        let Some(state) = self.tasks.get(child_id) else {
            return;
        };
        match &state.session {
            Some(session) => session.abort(),
            None => self.pending_stop = Some(child_id.to_owned()),
        }
        self.commit_cell(&Cell::Notice {
            text: format!("stopped {child_id}"),
        });
    }
}
