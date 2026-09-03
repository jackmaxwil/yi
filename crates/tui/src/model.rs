use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_types::model::{Effort, Model};

use crate::app::{App, Bottom};
use crate::cell::Cell;
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};

const MAX_VISIBLE: usize = 8;
const MORE: &str = "↑ More reasoning…";

/// The advanced tiers are reachable only from the picker's [`MORE`] row, so no
/// keystroke can bill the top tier by accident.
pub fn cycle_efforts(model: &Model) -> Vec<Effort> {
    model
        .supported_efforts()
        .into_iter()
        .filter(|effort| !effort.is_advanced())
        .collect()
}

fn advanced_efforts(model: &Model) -> Vec<Effort> {
    model
        .supported_efforts()
        .into_iter()
        .filter(|effort| effort.is_advanced())
        .collect()
}

/// An unadvertised `current` anchors to the clamp, never to a guessed rung.
pub fn step_effort(model: &Model, current: Effort, up: bool) -> Option<Effort> {
    let choices = cycle_efforts(model);
    let anchor = if choices.contains(&current) {
        current
    } else {
        model.clamp_effort(current)
    };
    let index = choices.iter().position(|effort| *effort == anchor)?;
    if up {
        choices.get(index.saturating_add(1)).copied()
    } else {
        index.checked_sub(1).and_then(|at| choices.get(at)).copied()
    }
}

pub fn advanced_hint(model: &Model) -> Option<String> {
    let names: Vec<String> = advanced_efforts(model)
        .iter()
        .map(Effort::to_string)
        .collect();
    (!names.is_empty()).then(|| format!("{} under /model → {MORE}", names.join(" and ")))
}

fn selector(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// `Some` once a model is picked: it, and whether advanced tiers are showing.
type Stage = Option<(Model, bool)>;

pub struct ModelPopup {
    stage: Stage,
    query: String,
    models: Vec<Model>,
    current: String,
    effort: Effort,
    selected: usize,
    /// Drained by the event loop, which owns the session.
    pub chosen: Option<(Model, Effort)>,
}

impl ModelPopup {
    pub fn new(models: Vec<Model>, current: &Model, effort: Effort, mru: &[String]) -> Self {
        let mut models = models;
        models.sort_by_cached_key(|model| {
            let key = selector(model);
            (
                mru.iter()
                    .position(|seen| *seen == key)
                    .unwrap_or(usize::MAX),
                key,
            )
        });
        let current = selector(current);
        let selected = models
            .iter()
            .position(|model| selector(model) == current)
            .unwrap_or(0);
        Self {
            stage: None,
            query: String::new(),
            models,
            current,
            effort,
            selected,
            chosen: None,
        }
    }

    fn filtered(&self) -> Vec<&Model> {
        let needle = self.query.to_lowercase();
        self.models
            .iter()
            .filter(|model| selector(model).to_lowercase().contains(&needle))
            .collect()
    }

    fn efforts(model: &Model, advanced: bool) -> (Vec<Effort>, bool) {
        if advanced {
            (advanced_efforts(model), false)
        } else {
            (cycle_efforts(model), !advanced_efforts(model).is_empty())
        }
    }

    fn rows(&self) -> usize {
        match &self.stage {
            None => self.filtered().len(),
            Some((model, advanced)) => {
                let (rows, more) = Self::efforts(model, *advanced);
                rows.len().saturating_add(usize::from(more))
            }
        }
    }

    fn enter(&mut self) -> PopupResult {
        match &self.stage {
            None => {
                let Some(model) = self.filtered().get(self.selected).map(|m| (*m).clone()) else {
                    return PopupResult::Close;
                };
                let (rows, more) = Self::efforts(&model, false);
                if rows.len() <= 1 && !more {
                    let effort = model.clamp_effort(self.effort);
                    self.chosen = Some((model, effort));
                    return PopupResult::Close;
                }
                let want = model.clamp_effort(self.effort);
                self.selected = rows.iter().position(|effort| *effort == want).unwrap_or(0);
                self.stage = Some((model, false));
                PopupResult::Open
            }
            Some((model, advanced)) => {
                let (rows, more) = Self::efforts(model, *advanced);
                if more && self.selected == rows.len() {
                    self.stage = Some((model.clone(), true));
                    self.selected = 0;
                    return PopupResult::Open;
                }
                if let Some(effort) = rows.get(self.selected) {
                    self.chosen = Some((model.clone(), *effort));
                }
                PopupResult::Close
            }
        }
    }
}

fn row(label: String, selected: bool, theme: &Theme) -> Line<'static> {
    let style = if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(Span::styled(label, style))
}

impl BottomView for ModelPopup {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        match &self.stage {
            None => {
                let mut out = vec![Line::from(vec![
                    Span::styled(
                        format!(" model {}", self.query),
                        Style::default().fg(theme.text),
                    ),
                    Span::styled("█", Style::default().fg(theme.accent)),
                ])];
                let cap = width.saturating_sub(4);
                let visible = self.filtered();
                let start = self.selected.saturating_sub(MAX_VISIBLE.saturating_sub(1));
                for (at, model) in visible.iter().enumerate().skip(start).take(MAX_VISIBLE) {
                    let label = selector(model);
                    let mark = if label == self.current { '›' } else { ' ' };
                    let label: String = label.chars().take(cap).collect();
                    out.push(row(format!(" {mark} {label}"), at == self.selected, theme));
                }
                out
            }
            Some((model, advanced)) => {
                let (rows, more) = Self::efforts(model, *advanced);
                let head = format!(
                    " reasoning for {}{}",
                    selector(model),
                    if *advanced { " · higher usage" } else { "" }
                );
                let mut out = vec![row(head, false, theme)];
                for (at, effort) in rows.iter().enumerate() {
                    out.push(row(format!("   {effort}"), at == self.selected, theme));
                }
                if more {
                    out.push(row(format!(" {MORE}"), self.selected == rows.len(), theme));
                }
                out
            }
        }
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        let last = self.rows().saturating_sub(1);
        let typing = self.stage.is_none();
        match key.code {
            KeyCodeValue::Esc => return PopupResult::Close,
            KeyCodeValue::Enter => return self.enter(),
            KeyCodeValue::Up => self.selected = self.selected.saturating_sub(1),
            KeyCodeValue::Down => self.selected = self.selected.saturating_add(1).min(last),
            KeyCodeValue::Backspace if typing => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCodeValue::Char(character) if typing => {
                self.query.push(character);
                self.selected = 0;
            }
            _ => {}
        }
        PopupResult::Open
    }
}

/// Invariant: the TUI's mirror of the session's selection. The event loop is
/// the only writer back to the session, draining `pending`.
pub struct Selection {
    pub model: Model,
    pub effort: Effort,
    pub mru: Vec<String>,
    pub pending: Option<(Model, Effort)>,
}

impl Selection {
    pub fn new(model: Model) -> Self {
        Self {
            effort: model.clamp_effort(Effort::default()),
            mru: vec![selector(&model)],
            model,
            pending: None,
        }
    }

    pub fn select(&mut self, model: Model, effort: Effort) {
        let key = selector(&model);
        self.mru.retain(|seen| *seen != key);
        self.mru.insert(0, key);
        self.mru.truncate(8);
        self.model = model.clone();
        self.effort = effort;
        self.pending = Some((model, effort));
    }

    /// `None` on one model used: the whole catalog is the picker's job.
    fn next_model(&self, forward: bool) -> Option<Model> {
        if self.mru.len() < 2 {
            return None;
        }
        let key = selector(&self.model);
        let at = self.mru.iter().position(|seen| *seen == key).unwrap_or(0);
        let len = self.mru.len();
        let step = if forward { 1 } else { len.saturating_sub(1) };
        let (provider, id) = self
            .mru
            .get(at.saturating_add(step) % len)?
            .split_once('/')?;
        yi_runtime::resolve_model(provider, id)
    }
}

impl App {
    pub(crate) fn open_model_picker(&mut self) {
        self.bottom = Some(Bottom::Model(Box::new(ModelPopup::new(
            yi_runtime::available_models(),
            &self.selection.model,
            self.selection.effort,
            &self.selection.mru,
        ))));
        self.scheduler.request();
    }

    pub(crate) fn step_effort(&mut self, up: bool) {
        let current = &self.selection;
        match step_effort(&current.model, current.effort, up) {
            Some(next) => self.select(self.selection.model.clone(), next),
            None => {
                let text = up
                    .then(|| advanced_hint(&current.model))
                    .flatten()
                    .unwrap_or_else(|| {
                        let bound = if up { "highest" } else { "lowest" };
                        format!(
                            "reasoning is already at the {bound} level ({})",
                            current.effort
                        )
                    });
                self.notice(text);
            }
        }
    }

    pub(crate) fn cycle_model(&mut self, forward: bool) {
        match self.selection.next_model(forward) {
            Some(model) => {
                let effort = model.clamp_effort(self.selection.effort);
                self.select(model, effort);
            }
            None => self.notice("only one model used this session - /model picks another"),
        }
    }

    pub(crate) fn select(&mut self, model: Model, effort: Effort) {
        self.selection.select(model, effort);
        self.scheduler.request();
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.commit_cell(&Cell::Notice { text: text.into() });
        self.scheduler.request();
    }
}
