use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use yi_runtime::AgentSession;
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::doc::{
    BlockedOn, Check, Plan, PlanState, PlanTier, Todo, TodoLabel, TodoState,
};

use crate::app::App;
use crate::cell::Cell;
use crate::colors::{Theme, name_accent};
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::tree::{PAGE, bottom_border, divider, gutter_prefix, row, top_border, window_lines};

const LEGEND: &str = "↑↓ move · type to filter · esc close";

pub enum PlanTreeResult {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    Head,
    Dim,
    Struck,
    Detail,
    Label,
    Ready,
    Warn,
    Ok,
    Bad,
    Named(Color),
}

impl Face {
    fn style(self, theme: &Theme) -> Style {
        match self {
            Self::Head => theme.accent_style().add_modifier(Modifier::BOLD),
            Self::Dim => theme.dim_style(),
            Self::Struck => theme.dim_style().add_modifier(Modifier::CROSSED_OUT),
            Self::Detail => theme.muted_style(),
            Self::Label => Style::default().fg(theme.text),
            Self::Ready => theme.accent_style(),
            Self::Warn => Style::default().fg(theme.warning),
            Self::Ok => Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::CROSSED_OUT),
            Self::Bad => Style::default().fg(theme.error),
            Self::Named(color) => Style::default().fg(color),
        }
    }
}

#[derive(Debug, Clone)]
struct Row {
    depth: usize,
    is_last: bool,
    filter: String,
    cells: Vec<(String, Face)>,
}

impl Row {
    fn spans(&self, theme: &Theme, background: Option<Color>) -> Vec<Span<'static>> {
        let prefix = gutter_prefix(self.depth, self.is_last);
        let gutter = (!prefix.is_empty()).then_some((prefix, Face::Dim));
        gutter
            .into_iter()
            .chain(self.cells.iter().cloned())
            .map(|(text, face)| {
                let style = face.style(theme);
                Span::styled(
                    text,
                    match background {
                        Some(color) => style.bg(color),
                        None => style,
                    },
                )
            })
            .collect()
    }
}

fn face(state: &TodoState, label: &str) -> (char, Face) {
    match state {
        TodoState::Pending => ('☐', Face::Dim),
        TodoState::Running { .. } => ('◆', Face::Named(name_accent(label))),
        TodoState::Blocked { .. } => ('☐', Face::Warn),
        TodoState::Done { .. } => ('☑', Face::Ok),
        TodoState::Failed { .. } => ('✗', Face::Bad),
        TodoState::Abandoned => ('☐', Face::Struck),
        TodoState::Other(_) => ('?', Face::Dim),
    }
}

fn plan_state(state: &PlanState) -> String {
    match state {
        PlanState::Active => "active".to_owned(),
        PlanState::Done => "done".to_owned(),
        PlanState::Superseded { by } => format!("superseded by v{}", by.0),
        PlanState::Abandoned => "abandoned".to_owned(),
        PlanState::Other(tag) => tag.clone(),
    }
}

fn accept_text(check: &Check) -> &str {
    match check {
        Check::Command(command) => command,
        Check::Stated(stated) => stated,
        Check::Other(other) => other,
    }
}

fn blocked_text(on: &BlockedOn) -> String {
    match on {
        BlockedOn::Child(agent) => format!("child {agent}"),
        BlockedOn::User => "user".to_owned(),
        BlockedOn::External { probe: _ } => "external".to_owned(),
        BlockedOn::Other(tag) => tag.clone(),
    }
}

fn detail(todo: &Todo) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &todo.state {
        TodoState::Pending | TodoState::Abandoned => {}
        TodoState::Running { by } => parts.push(format!("by {by}")),
        TodoState::Blocked { on, note } => {
            parts.push(blocked_text(on));
            if !note.is_empty() {
                parts.push(note.clone());
            }
        }
        TodoState::Done { output } => {
            if let Some(url) = output {
                parts.push(url.to_string());
            }
        }
        TodoState::Failed { cause, last } => {
            parts.push(cause.clone());
            if let Some(url) = last {
                parts.push(format!("last: {url}"));
            }
        }
        TodoState::Other(tag) => parts.push(tag.clone()),
    }
    if let Some(delegation) = &todo.delegation {
        parts.push(format!(
            "→ {}",
            delegation.spec.role.as_deref().unwrap_or("child")
        ));
        parts.push(accept_text(&delegation.accept).to_owned());
    }
    parts.join(" · ")
}

fn owned_by(sub: &Plan, plan: &Plan, todo: &TodoLabel) -> bool {
    match &sub.tier {
        PlanTier::Sub { parent } => parent.plan == plan.id && &parent.todo == todo,
        PlanTier::Root | PlanTier::Other { .. } => false,
    }
}

fn push_plan(rows: &mut Vec<Row>, plan: &Plan, depth: usize, subplans: &[Plan]) {
    let head = format!(
        "{} v{} · {} · {}",
        plan.id,
        plan.version.0,
        plan.goal,
        plan_state(&plan.state)
    );
    rows.push(Row {
        depth,
        is_last: true,
        filter: plan.id.to_string(),
        cells: vec![(head, Face::Head)],
    });
    let ready: Vec<&TodoLabel> = plan.ready().iter().map(|todo| &todo.label).collect();
    let last = plan.todos.len().saturating_sub(1);
    for (index, todo) in plan.todos.iter().enumerate() {
        let label = todo.label.to_string();
        let (glyph, face) = face(&todo.state, &label);
        let filter = format!("{label} {}", plan.id);
        let mut cells = vec![
            (format!("{glyph} "), face),
            (
                label,
                if ready.contains(&&todo.label) {
                    Face::Ready
                } else {
                    Face::Label
                },
            ),
        ];
        match detail(todo) {
            detail if detail.is_empty() => {}
            detail => cells.push((format!("  {detail}"), Face::Detail)),
        }
        rows.push(Row {
            depth: depth.saturating_add(1),
            is_last: index == last,
            filter: filter.clone(),
            cells,
        });
        if !todo.after.is_empty() {
            let after: Vec<&str> = todo.after.iter().map(TodoLabel::as_str).collect();
            rows.push(Row {
                depth: depth.saturating_add(2),
                is_last: true,
                filter: filter.clone(),
                cells: vec![(format!("after: {}", after.join(", ")), Face::Dim)],
            });
        }
        for sub in subplans
            .iter()
            .filter(|sub| owned_by(sub, plan, &todo.label))
        {
            push_plan(rows, sub, depth.saturating_add(2), &[]);
        }
    }
}

/// The ledger as the runtime reads it: every plan, todo, state, and `after`
/// edge, flattened once at open because [`Plan::ready`] walks the whole
/// document and a draw runs per frame.
pub struct PlanTreeView {
    title: String,
    rows: Vec<Row>,
    pub selected: usize,
    pub query: String,
}

impl PlanTreeView {
    pub fn new(plan: &Plan, subplans: &[Plan]) -> Self {
        let mut rows = Vec::new();
        push_plan(&mut rows, plan, 0, subplans);
        Self {
            title: yi_runtime::plan::summary_line(plan),
            rows,
            selected: 0,
            query: String::new(),
        }
    }

    fn visible(&self) -> Vec<(usize, &Row)> {
        let needle = self.query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| needle.is_empty() || row.filter.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn lines(&self, width: usize, theme: &Theme, max_rows: usize) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(4);
        let mut out = vec![top_border(width, &self.title, theme)];
        if !self.query.is_empty() {
            out.push(row(
                vec![Span::styled(
                    format!("Search: {}", self.query),
                    theme.accent_style(),
                )],
                inner,
                theme,
                None,
            ));
        }
        out.push(divider(width, theme));
        for spans in self.rows_lines(inner, theme, max_rows.max(1)) {
            out.push(row(spans, inner, theme, None));
        }
        out.push(row(
            vec![Span::styled(LEGEND.to_owned(), theme.muted_style())],
            inner,
            theme,
            None,
        ));
        out.push(bottom_border(width, theme));
        out
    }

    fn rows_lines(&self, inner: usize, theme: &Theme, max_rows: usize) -> Vec<Vec<Span<'static>>> {
        let visible = self.visible();
        if visible.is_empty() {
            return vec![vec![Span::styled(
                "No todos match".to_owned(),
                theme.muted_style(),
            )]];
        }
        let position = visible
            .iter()
            .position(|(index, _)| *index == self.selected)
            .unwrap_or(0);
        window_lines(
            visible.len(),
            position,
            inner,
            max_rows,
            theme,
            |index, background| match visible.get(index) {
                Some((_, row)) => row.spans(theme, background),
                None => Vec::new(),
            },
        )
    }

    pub fn handle_key(&mut self, key: &SingleKey) -> PlanTreeResult {
        let visible: Vec<usize> = self.visible().iter().map(|(index, _)| *index).collect();
        let position = visible
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        let at = |index: usize| visible.get(index).copied();
        match key.code {
            KeyCodeValue::Esc | KeyCodeValue::Enter => return PlanTreeResult::Close,
            KeyCodeValue::Up => self.select(at(position.saturating_sub(1))),
            KeyCodeValue::Down => self.select(at(position.saturating_add(1))),
            KeyCodeValue::PageUp => {
                self.select(at(position.saturating_sub(PAGE)).or(visible.first().copied()));
            }
            KeyCodeValue::PageDown => {
                self.select(at(position.saturating_add(PAGE)).or(visible.last().copied()));
            }
            KeyCodeValue::Home => self.select(visible.first().copied()),
            KeyCodeValue::End => self.select(visible.last().copied()),
            KeyCodeValue::Backspace => {
                self.query.pop();
                self.reselect();
            }
            KeyCodeValue::Char(c) if !key.ctrl && !key.alt => {
                self.query.push(c);
                self.reselect();
            }
            _ => {}
        }
        PlanTreeResult::Open
    }

    fn select(&mut self, index: Option<usize>) {
        if let Some(index) = index {
            self.selected = index;
        }
    }

    /// Invariant: the selection is an index into every row, so a filter that
    /// hides it must move it or the cursor renders nowhere.
    fn reselect(&mut self) {
        let visible = self.visible();
        if visible.iter().all(|(index, _)| *index != self.selected)
            && let Some((index, _)) = visible.first()
        {
            self.selected = *index;
        }
    }
}

fn subplans_of(plan: &Plan, plans_dir: &std::path::Path) -> Vec<Plan> {
    let ids: Vec<_> = plan
        .todos
        .iter()
        .filter_map(|todo| todo.subplan.clone())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(store) = PlanStore::open(plans_dir.to_path_buf()) else {
        return Vec::new();
    };
    ids.iter()
        .filter_map(|id| store.read(id).ok())
        .map(|file| file.plan)
        .collect()
}

pub(crate) fn open_plan_tree(app: &mut App, session: &AgentSession) {
    let notice = |app: &mut App, text: String| app.commit_cell(&Cell::Notice { text });
    let Some(service) = session.plan_service() else {
        notice(
            app,
            "/plantree: no plan service is attached to this session".to_owned(),
        );
        return;
    };
    match service.read_plan() {
        Err(error) => notice(app, format!("/plantree: {error}")),
        Ok(plan) => {
            let subplans = subplans_of(&plan, service.plans_dir());
            app.plan_tree = Some(PlanTreeView::new(&plan, &subplans));
        }
    }
    app.scheduler.request();
}
