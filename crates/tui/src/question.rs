//! The card a root's `ask_user` call opens in place of the composer: one page per question.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Bottom};
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};
use crate::wrap::{fit, wrap_line};

const PREVIEW_LINES: usize = 5;
const SKIPPED: &str = "Skip: go with your recommendation and say which.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    label: String,
    description: Option<String>,
    preview: Option<String>,
}

impl Choice {
    fn from_value(value: &Value) -> Option<Self> {
        let text = |key: &str| {
            let text = value.get(key)?.as_str()?.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        let label = match value {
            Value::String(label) => Some(label.trim().to_owned()),
            _ => text("label"),
        }
        .filter(|label| !label.is_empty())?;
        let preview = value
            .get("preview")
            .and_then(Value::as_str)
            .filter(|preview| !preview.trim().is_empty())
            .map(|preview| preview.trim_end().to_owned());
        Some(Self {
            label,
            description: text("description"),
            preview,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Question {
    prompt: String,
    choices: Vec<Choice>,
}

impl Question {
    /// With no option it is an open question: the card is the Other row alone, already typing.
    pub(crate) fn from_args(args: &Value) -> Option<Self> {
        let prompt = args.get("question")?.as_str()?.trim().to_owned();
        let choices: Vec<Choice> = args
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Choice::from_value)
            .collect();
        (!prompt.is_empty()).then_some(Self { prompt, choices })
    }
}

pub(crate) struct QuestionView {
    questions: Vec<Question>,
    page: usize,
    cursor: usize,
    typing: bool,
    other: String,
    answers: Vec<String>,
}

impl QuestionView {
    pub(crate) fn new(questions: Vec<Question>) -> Self {
        let open = questions
            .first()
            .is_some_and(|question| question.choices.is_empty());
        Self {
            questions,
            page: 0,
            cursor: 0,
            typing: open,
            other: String::new(),
            answers: Vec::new(),
        }
    }

    fn current(&self) -> Option<&Question> {
        self.questions.get(self.page)
    }

    /// The last page's answer closes the card as one message, so the transcript keeps the
    /// user's words as the single record of the pick.
    fn answer(&mut self, text: String) -> PopupResult {
        self.answers.push(text);
        self.page = self.page.saturating_add(1);
        self.cursor = 0;
        self.other.clear();
        if let Some(next) = self.current() {
            self.typing = next.choices.is_empty();
            return PopupResult::Open;
        }
        let reply = match self.answers.as_slice() {
            [only] => only.clone(),
            many => self
                .questions
                .iter()
                .zip(many)
                .map(|(question, answer)| format!("{} → {answer}", question.prompt))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        PopupResult::Insert(reply)
    }

    /// Row `at` past the last option is Other, which takes typing instead of answering.
    fn pick(&mut self, at: usize) -> PopupResult {
        let label = self
            .current()
            .and_then(|question| question.choices.get(at))
            .map(|choice| choice.label.clone());
        match label {
            Some(label) => self.answer(label),
            None => {
                self.cursor = at;
                self.typing = true;
                PopupResult::Open
            }
        }
    }
}

fn keyed(mut left: Vec<Span<'static>>, key: usize, width: usize, theme: &Theme) -> Line<'static> {
    let key = key.to_string();
    let used: usize = left.iter().map(|span| span.content.width()).sum();
    let gap = width.saturating_sub(used.saturating_add(key.width()).saturating_add(1));
    left.push(Span::raw(" ".repeat(gap)));
    left.push(Span::styled(key, theme.dim_style()));
    fit(Line::from(left), width)
}

fn preview(choice: &Choice, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(text) = &choice.preview else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let mut rows: Vec<Line<'static>> = all
        .iter()
        .take(PREVIEW_LINES)
        .map(|raw| {
            let line = Line::from(Span::styled(format!("     │ {raw}"), theme.dim_style()));
            fit(line, width)
        })
        .collect();
    if all.len() > PREVIEW_LINES {
        let cut = format!(
            "     [… showing {PREVIEW_LINES} of {} lines; the card shows at most {PREVIEW_LINES}; ask for the rest in the composer]",
            all.len()
        );
        rows.push(fit(
            Line::from(Span::styled(cut, theme.muted_style())),
            width,
        ));
    }
    rows
}

impl BottomView for QuestionView {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let Some(question) = self.current() else {
            return Vec::new();
        };
        let chip = match self.questions.len() {
            1 => " ? ".to_owned(),
            total => format!(" {}/{total} ", self.page.saturating_add(1)),
        };
        let bold = Modifier::BOLD;
        let head = Line::from(vec![
            Span::styled(chip.clone(), theme.accent_style().add_modifier(bold)),
            Span::styled(
                question.prompt.clone(),
                Style::default().fg(theme.text).add_modifier(bold),
            ),
        ]);
        let mut out = wrap_line(&head, width, &" ".repeat(chip.width()));
        let others = question.choices.len();
        let label = |at: usize, text: &str| {
            let on = at == self.cursor;
            let style = match on {
                true => theme.accent_style().add_modifier(bold),
                false => Style::default().fg(theme.text),
            };
            let mark = if on { "▸" } else { " " };
            Span::styled(format!(" {mark} {text}"), style)
        };
        for (at, choice) in question.choices.iter().enumerate() {
            let mut row = vec![label(at, &choice.label)];
            if at == 0 {
                row.push(Span::styled(" (recommended)", theme.muted_style()));
            }
            out.push(keyed(row, at.saturating_add(1), width, theme));
            if let Some(description) = &choice.description {
                let text = format!("     {description}");
                out.push(fit(
                    Line::from(Span::styled(text, theme.muted_style())),
                    width,
                ));
            }
            if at == self.cursor {
                out.extend(preview(choice, width, theme));
            }
        }
        let typed = match self.typing {
            true => Span::styled(format!("{}█", self.other), Style::default().fg(theme.text)),
            false => Span::styled("type your own answer", theme.dim_style()),
        };
        out.push(keyed(
            vec![
                label(others, if others == 0 { "Answer  " } else { "Other  " }),
                typed,
            ],
            others.saturating_add(1),
            width,
            theme,
        ));
        let hint = if self.typing {
            "   ⏎ send · esc back"
        } else {
            "   number or ↑↓ then ⏎ · s skip · esc close"
        };
        out.push(fit(
            Line::from(Span::styled(hint, theme.dim_style())),
            width,
        ));
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        let Some(others) = self.current().map(|question| question.choices.len()) else {
            return PopupResult::Close;
        };
        if self.typing {
            return match key.code {
                KeyCodeValue::Enter => {
                    let text = self.other.trim().to_owned();
                    if text.is_empty() {
                        PopupResult::Open
                    } else {
                        self.answer(text)
                    }
                }
                KeyCodeValue::Esc => {
                    self.typing = false;
                    PopupResult::Open
                }
                KeyCodeValue::Backspace => {
                    if self.other.pop().is_none() {
                        self.typing = false;
                    }
                    PopupResult::Open
                }
                KeyCodeValue::Space => {
                    self.other.push(' ');
                    PopupResult::Open
                }
                KeyCodeValue::Char(c) if !key.ctrl && !key.alt => {
                    self.other.push(c);
                    PopupResult::Open
                }
                _ => PopupResult::Open,
            };
        }
        match key.code {
            KeyCodeValue::Esc => PopupResult::Close,
            KeyCodeValue::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Down | KeyCodeValue::Tab => {
                self.cursor = self.cursor.saturating_add(1).min(others);
                PopupResult::Open
            }
            KeyCodeValue::Enter => self.pick(self.cursor),
            KeyCodeValue::Char('s') => PopupResult::Insert(SKIPPED.to_owned()),
            KeyCodeValue::Char(c) => {
                let nth = c
                    .to_digit(10)
                    .and_then(|digit| usize::try_from(digit).ok())
                    .and_then(|digit| digit.checked_sub(1));
                match nth {
                    Some(at) if at <= others => self.pick(at),
                    _ => PopupResult::Open,
                }
            }
            _ => PopupResult::Open,
        }
    }
}

impl App {
    pub(crate) fn note_question(&mut self, tool_name: &str, args: &Value) {
        if tool_name == "ask_user"
            && let Some(question) = Question::from_args(args)
        {
            self.asked.push(question);
        }
    }

    /// Opens at the turn's end: `ask_user` ends the turn, so the card never races a stream.
    pub(crate) fn open_questions(&mut self) {
        let asked = std::mem::take(&mut self.asked);
        if self.bottom.is_none() && !asked.is_empty() {
            self.bottom = Some(Bottom::Question(QuestionView::new(asked)));
            self.scheduler.request();
        }
    }
}
