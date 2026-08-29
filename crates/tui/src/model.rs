use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_types::model::{Effort, Model};

use crate::app::{App, Bottom};
use crate::cell::Cell;
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};

const MAX_VISIBLE: usize = 8;

/// Levels a cycle shortcut walks. The advanced tiers are reachable only from
/// the picker's `More reasoning…` row, so a keystroke cannot bill the top tier.
pub fn cycle_efforts(model: &Model) -> Vec<Effort> {
    let supported: Vec<Effort> = model
        .supported_efforts()
        .into_iter()
        .filter(|effort| !effort.is_advanced())
        .collect();
    if supported.is_empty() {
        model.supported_efforts()
    } else {
        supported
    }
}

/// The next level up or down within [`cycle_efforts`], or `None` at the bound.
/// An effort the model does not advertise anchors to the model's own default
/// rather than guessing a rung it never had.
pub fn step_effort(model: &Model, current: Effort, up: bool) -> Option<Effort> {
    let choices = cycle_efforts(model);
    let anchored = if choices.contains(&current) {
        current
    } else {
        model.clamp_effort(current)
    };
    let index = choices.iter().position(|effort| *effort == anchored)?;
    if up {
        choices.get(index.saturating_add(1)).copied()
    } else {
        index.checked_sub(1).and_then(|prev| choices.get(prev)).copied()
    }
}

/// Where the advanced tiers live, for the message a bounded cycle prints.
pub fn advanced_hint(model: &Model) -> Option<String> {
    let advanced: Vec<String> = model
        .supported_efforts()
        .into_iter()
        .filter(|effort| effort.is_advanced())
        .map(|effort| effort.to_string())
        .collect();
    (!advanced.is_empty())
        .then(|| format!("{} available under /model → More reasoning…", advanced.join(" and ")))
}

enum Stage {
    Models,
    Efforts { model: Box<Model>, advanced: bool },
}

pub struct ModelPopup {
    stage: Stage,
    query: String,
    models: Vec<Model>,
    current: (String, String),
    current_effort: Effort,
    selected: usize,
    /// Drained by the event loop and applied to the session.
    pub chosen: Option<(Model, Effort)>,
}

impl ModelPopup {
    pub fn new(
        models: Vec<Model>,
        current: &Model,
        current_effort: Effort,
        mru: &[(String, String)],
    ) -> Self {
        let mut models = models;
        models.sort_by_cached_key(|model| {
            let key = (model.provider.clone(), model.id.clone());
            let rank = mru.iter().position(|entry| *entry == key);
            (rank.unwrap_or(usize::MAX), key)
        });
        let current = (current.provider.clone(), current.id.clone());
        let selected = models
            .iter()
            .position(|model| (model.provider.clone(), model.id.clone()) == current)
            .unwrap_or(0);
        Self {
            stage: Stage::Models,
            query: String::new(),
            models,
            current,
            current_effort,
            selected,
            chosen: None,
        }
    }

    fn filtered(&self) -> Vec<&Model> {
        let needle = self.query.to_lowercase();
        self.models
            .iter()
            .filter(|model| {
                needle.is_empty() || selector(model).to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn effort_rows(model: &Model, advanced: bool) -> Vec<Effort> {
        if advanced {
            model
                .supported_efforts()
                .into_iter()
                .filter(|effort| effort.is_advanced())
                .collect()
        } else {
            cycle_efforts(model)
        }
    }

    fn pick_model(&mut self) -> PopupResult {
        let Some(model) = self.filtered().get(self.selected).map(|model| (*model).clone())
        else {
            return PopupResult::Close;
        };
        if model.supported_efforts().len() <= 1 {
            let effort = model.clamp_effort(self.current_effort);
            self.chosen = Some((model, effort));
            return PopupResult::Close;
        }
        self.selected = Self::effort_rows(&model, false)
            .iter()
            .position(|effort| *effort == model.clamp_effort(self.current_effort))
            .unwrap_or(0);
        self.stage = Stage::Efforts {
            model: Box::new(model),
            advanced: false,
        };
        PopupResult::Open
    }

    fn pick_effort(&mut self) -> PopupResult {
        let Stage::Efforts { model, advanced } = &self.stage else {
            return PopupResult::Close;
        };
        let rows = Self::effort_rows(model, *advanced);
        let more_row = !*advanced && rows.len() < model.supported_efforts().len();
        if more_row && self.selected == rows.len() {
            let model = model.clone();
            self.selected = 0;
            self.stage = Stage::Efforts {
                model,
                advanced: true,
            };
            return PopupResult::Open;
        }
        match rows.get(self.selected) {
            Some(effort) => {
                self.chosen = Some(((**model).clone(), *effort));
                PopupResult::Close
            }
            None => PopupResult::Close,
        }
    }

    fn rows(&self) -> usize {
        match &self.stage {
            Stage::Models => self.filtered().len(),
            Stage::Efforts { model, advanced } => {
                let rows = Self::effort_rows(model, *advanced).len();
                if !*advanced && rows < model.supported_efforts().len() {
                    rows.saturating_add(1)
                } else {
                    rows
                }
            }
        }
    }
}

fn selector(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn glyph(effort: Effort) -> &'static str {
    match effort {
        Effort::Off => "·",
        Effort::Minimal => "◔",
        Effort::Low => "◑",
        Effort::Medium => "◕",
        Effort::High => "●",
        Effort::XHigh | Effort::Max => "◉",
    }
}

impl BottomView for ModelPopup {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        let selected_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let plain = Style::default().fg(theme.text);
        let dim = Style::default().fg(theme.dim);
        match &self.stage {
            Stage::Models => {
                out.push(Line::from(vec![
                    Span::styled(format!(" model {}", self.query), plain),
                    Span::styled("█", Style::default().fg(theme.accent)),
                ]));
                let filtered = self.filtered();
                let start = self.selected.saturating_sub(MAX_VISIBLE.saturating_sub(1));
                for (index, model) in filtered.iter().enumerate().skip(start).take(MAX_VISIBLE) {
                    let is_current =
                        (model.provider.clone(), model.id.clone()) == self.current;
                    let mark = if is_current { "›" } else { " " };
                    let label = selector(model);
                    let label = if label.len() > width.saturating_sub(6) {
                        label.chars().take(width.saturating_sub(6)).collect()
                    } else {
                        label
                    };
                    out.push(Line::from(vec![
                        Span::styled(
                            format!(" {mark} {label}"),
                            if index == self.selected { selected_style } else { plain },
                        ),
                    ]));
                }
            }
            Stage::Efforts { model, advanced } => {
                out.push(Line::from(Span::styled(
                    format!(
                        " reasoning for {}{}",
                        selector(model),
                        if *advanced { " · higher usage" } else { "" }
                    ),
                    plain,
                )));
                let rows = Self::effort_rows(model, *advanced);
                for (index, effort) in rows.iter().enumerate() {
                    out.push(Line::from(Span::styled(
                        format!(" {} {effort}", glyph(*effort)),
                        if index == self.selected { selected_style } else { plain },
                    )));
                }
                if !*advanced && rows.len() < model.supported_efforts().len() {
                    out.push(Line::from(Span::styled(
                        " ↑ More reasoning…".to_owned(),
                        if self.selected == rows.len() { selected_style } else { dim },
                    )));
                }
            }
        }
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        let rows = self.rows();
        match key.code {
            KeyCodeValue::Esc => PopupResult::Close,
            KeyCodeValue::Up => {
                self.selected = self.selected.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(rows.saturating_sub(1));
                PopupResult::Open
            }
            KeyCodeValue::Enter => match self.stage {
                Stage::Models => self.pick_model(),
                Stage::Efforts { .. } => self.pick_effort(),
            },
            KeyCodeValue::Backspace => {
                if matches!(self.stage, Stage::Models) {
                    self.query.pop();
                    self.selected = 0;
                }
                PopupResult::Open
            }
            KeyCodeValue::Char(character) => {
                if matches!(self.stage, Stage::Models) {
                    self.query.push(character);
                    self.selected = 0;
                }
                PopupResult::Open
            }
            _ => PopupResult::Open,
        }
    }
}

/// Invariant: the TUI's mirror of the session's selection. The event loop is
/// the only writer back to the session, draining `pending`.
pub struct Selection {
    pub model: Model,
    pub effort: Effort,
    /// Most-recently-used models, newest first — the `ctrl-p` cycle scope.
    pub mru: Vec<(String, String)>,
    pub pending: Option<(Model, Effort)>,
}

impl Selection {
    pub fn new(model: Model) -> Self {
        Self {
            effort: model.clamp_effort(Effort::default()),
            mru: vec![(model.provider.clone(), model.id.clone())],
            model,
            pending: None,
        }
    }

    /// Records the choice for display and queues it for the event loop.
    pub fn select(&mut self, model: Model, effort: Effort) {
        let key = (model.provider.clone(), model.id.clone());
        self.mru.retain(|entry| *entry != key);
        self.mru.insert(0, key);
        self.mru.truncate(8);
        self.model = model.clone();
        self.effort = effort;
        self.pending = Some((model, effort));
    }

    /// The next model in the session's own history, or `None` when only one
    /// has been used — the whole catalog is the picker's job, not a cycle's.
    pub fn next_model(&self, forward: bool) -> Option<Model> {
        if self.mru.len() < 2 {
            return None;
        }
        let key = (self.model.provider.clone(), self.model.id.clone());
        let at = self.mru.iter().position(|entry| *entry == key).unwrap_or(0);
        let len = self.mru.len();
        let next = if forward {
            at.saturating_add(1) % len
        } else {
            at.saturating_add(len.saturating_sub(1)) % len
        };
        let (provider, id) = self.mru.get(next)?;
        yi_runtime::resolve_model(provider, id)
    }
}

impl App {
    /// Opens the picker over every catalog model, current selection first.
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
                        format!(
                            "reasoning is already at the {} level ({})",
                            if up { "highest" } else { "lowest" },
                            current.effort
                        )
                    });
                self.commit_cell(&Cell::Notice { text });
                self.scheduler.request();
            }
        }
    }

    pub(crate) fn cycle_model(&mut self, forward: bool) {
        match self.selection.next_model(forward) {
            Some(model) => {
                let effort = model.clamp_effort(self.selection.effort);
                self.select(model, effort);
            }
            None => {
                self.commit_cell(&Cell::Notice {
                    text: "only one model used this session - /model picks another".to_owned(),
                });
                self.scheduler.request();
            }
        }
    }

    pub(crate) fn select(&mut self, model: Model, effort: Effort) {
        self.selection.select(model, effort);
        self.scheduler.request();
    }
}
