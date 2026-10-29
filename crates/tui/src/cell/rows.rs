use ratatui::text::Line;

use crate::colors::{ColorTier, Theme};
use crate::markdown;

/// Incident: every running card rendered its whole answer each frame to keep four rows, 12 ms a
/// frame for eight cards in the dist build. The rows are kept until the answer or width changes.
#[derive(Default)]
pub struct AnswerRows(std::sync::Mutex<Option<KeptRows>>);

type KeptRows = (usize, ColorTier, bool, String, Vec<Line<'static>>);

impl AnswerRows {
    pub(super) fn window<R>(
        &self,
        answer: &str,
        width: usize,
        theme: &Theme,
        pick: impl FnOnce(&[Line<'static>]) -> R,
    ) -> R {
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fresh = held.as_ref().is_some_and(|(w, tier, dark, source, _)| {
            (*w, *tier, *dark) == (width, theme.tier, theme.dark) && source == answer
        });
        if !fresh {
            let rows = markdown::render(answer, width.saturating_sub(6), theme).into_iter();
            let rows = rows.skip_while(|line| line.spans.is_empty()).collect();
            *held = Some((width, theme.tier, theme.dark, answer.to_owned(), rows));
        }
        pick(held.as_ref().map_or(&[], |(.., rows)| rows.as_slice()))
    }
}
