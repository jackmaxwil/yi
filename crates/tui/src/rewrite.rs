//! The composer's rewrite control: the position it sits at (off or a length) and how far a
//! draft is on its way from typed to sent.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use yi_types::config::RewriteLength;

use crate::colors::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Draft,
    Rewriting { ask: u64, original: String },
    Proposed { original: String },
    AsTyped { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Draft,
    Rewriting,
    Proposed,
    AsTyped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Enter,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    Send,
    Ask,
    Restore,
    Hold,
    Pass,
}

/// Invariant: every key the control answers is one row; a pair with no row is [`Effect::Pass`].
#[rustfmt::skip]
const TABLE: [(Kind, Event, Kind, Effect); 6] = [
    (Kind::Draft,     Event::Enter,  Kind::Rewriting, Effect::Ask),
    (Kind::Rewriting, Event::Enter,  Kind::Rewriting, Effect::Hold),
    (Kind::Rewriting, Event::Escape, Kind::AsTyped,   Effect::Restore),
    (Kind::Proposed,  Event::Enter,  Kind::Draft,     Effect::Send),
    (Kind::Proposed,  Event::Escape, Kind::AsTyped,   Effect::Restore),
    (Kind::AsTyped,   Event::Enter,  Kind::Draft,     Effect::Send),
];

const ORDER: [Option<RewriteLength>; 4] = [
    None,
    Some(RewriteLength::Short),
    Some(RewriteLength::Medium),
    Some(RewriteLength::Long),
];

fn label(position: Option<RewriteLength>) -> &'static str {
    match position {
        None => "off",
        Some(RewriteLength::Short) => "short",
        Some(RewriteLength::Medium) => "medium",
        Some(RewriteLength::Long) => "long",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rewrite {
    available: bool,
    position: Option<RewriteLength>,
    phase: Phase,
    asks: u64,
}

impl Default for Rewrite {
    fn default() -> Self {
        Self {
            available: false,
            position: None,
            phase: Phase::Draft,
            asks: 0,
        }
    }
}

impl Rewrite {
    pub(crate) fn ready(start: RewriteLength) -> Self {
        Self {
            available: true,
            position: Some(start),
            ..Self::default()
        }
    }

    fn kind(&self) -> Kind {
        match self.phase {
            Phase::Draft => Kind::Draft,
            Phase::Rewriting { .. } => Kind::Rewriting,
            Phase::Proposed { .. } => Kind::Proposed,
            Phase::AsTyped { .. } => Kind::AsTyped,
        }
    }

    /// Invariant: a phase speaks for the text it was made about; a box that no longer holds it
    /// (cleared, or the typed words edited) is a new draft.
    fn settle(&mut self, held: &str) {
        let stale = match &self.phase {
            Phase::Draft => false,
            Phase::Rewriting { original, .. } => original != held,
            Phase::Proposed { .. } => held.trim().is_empty(),
            Phase::AsTyped { text } => text != held,
        };
        if stale {
            self.phase = Phase::Draft;
        }
    }

    pub(crate) fn locked(&mut self, held: &str) -> bool {
        self.settle(held);
        matches!(self.phase, Phase::Rewriting { .. })
    }

    pub(crate) fn cycle(&mut self) {
        if !self.available || matches!(self.phase, Phase::Rewriting { .. }) {
            return;
        }
        let at = ORDER.iter().position(|slot| *slot == self.position);
        let next = at.map_or(0, |at| at.saturating_add(1) % ORDER.len());
        self.position = ORDER.get(next).copied().flatten();
    }

    pub(crate) fn step(&mut self, event: Event, held: &str, eligible: bool) -> Effect {
        self.settle(held);
        let from = self.kind();
        let Some((_, _, to, effect)) = (TABLE.iter()).find(|row| row.0 == from && row.1 == event)
        else {
            return Effect::Pass;
        };
        if from == Kind::Draft && !(eligible && self.position.is_some()) {
            return Effect::Send;
        }
        if *effect == Effect::Hold {
            return Effect::Hold;
        }
        let original = match std::mem::replace(&mut self.phase, Phase::Draft) {
            Phase::Rewriting { original, .. } | Phase::Proposed { original } => original,
            Phase::Draft | Phase::AsTyped { .. } => held.to_owned(),
        };
        self.phase = match to {
            Kind::Draft => Phase::Draft,
            Kind::Rewriting => {
                self.asks = self.asks.saturating_add(1);
                Phase::Rewriting {
                    ask: self.asks,
                    original,
                }
            }
            Kind::Proposed => Phase::Proposed { original },
            Kind::AsTyped => Phase::AsTyped { text: original },
        };
        *effect
    }

    pub(crate) fn asking(&self) -> Option<(u64, RewriteLength)> {
        match (&self.phase, self.position) {
            (Phase::Rewriting { ask, .. }, Some(length)) => Some((*ask, length)),
            _ => None,
        }
    }

    pub(crate) fn typed(&self) -> Option<&str> {
        match &self.phase {
            Phase::AsTyped { text } => Some(text),
            _ => None,
        }
    }

    pub(crate) fn answer(
        &mut self,
        ask: u64,
        outcome: Result<String, String>,
        held: &str,
    ) -> Option<Result<String, String>> {
        self.settle(held);
        let Phase::Rewriting {
            ask: open,
            original,
        } = &self.phase
        else {
            return None;
        };
        if *open != ask {
            return None;
        }
        let original = original.clone();
        self.phase = match &outcome {
            Ok(_) => Phase::Proposed { original },
            Err(_) => Phase::AsTyped { text: original },
        };
        Some(outcome)
    }

    pub(crate) fn marker(
        &mut self,
        theme: &Theme,
        width: usize,
        held: &str,
    ) -> Option<Line<'static>> {
        if !self.available {
            return None;
        }
        self.settle(held);
        let dim = theme.dim_style();
        let note = |text: &str| Line::from(Span::styled(format!(" {text} "), dim));
        Some(match &self.phase {
            Phase::Rewriting { .. } => note("improving… · esc keeps your words"),
            Phase::Proposed { .. } => note("improved · enter sends · esc restores"),
            Phase::AsTyped { .. } => note("as typed · enter sends"),
            Phase::Draft if width < 52 => note(&format!("improve: {}", label(self.position))),
            Phase::Draft => Line::from(
                (self.draft_cells().into_iter())
                    .map(|(shown, slot)| match slot {
                        Some(index) if ORDER.get(index) == Some(&self.position) => {
                            Span::styled(shown, theme.accent_style().add_modifier(Modifier::BOLD))
                        }
                        _ => Span::styled(shown, dim),
                    })
                    .collect::<Vec<_>>(),
            ),
        })
    }

    /// The idle control as text cells, each slot tagged with its place in [`ORDER`]: what is
    /// drawn and where a press lands both read this one list.
    fn draft_cells(&self) -> Vec<(String, Option<usize>)> {
        let mut cells = vec![(" improve: ".to_owned(), None)];
        for (index, slot) in ORDER.iter().enumerate() {
            if index > 0 {
                cells.push((" · ".to_owned(), None));
            }
            let shown = match *slot == self.position {
                true => format!("[{}]", label(*slot)),
                false => label(*slot).to_owned(),
            };
            cells.push((shown, Some(index)));
        }
        cells.push((" ".to_owned(), None));
        cells
    }

    /// Where each position sits in the border title: column offset, width and position. Only
    /// the wide idle control has them; the narrow form and the working notes are not pickable.
    pub(crate) fn slots(&self, width: usize) -> Vec<(u16, u16, Option<RewriteLength>)> {
        if !self.available || width < 52 || !matches!(self.phase, Phase::Draft) {
            return Vec::new();
        }
        let mut at = 0_u16;
        let mut found = Vec::new();
        for (shown, slot) in self.draft_cells() {
            let wide = u16::try_from(shown.chars().count()).unwrap_or(0);
            if let Some(position) = slot.and_then(|index| ORDER.get(index)) {
                found.push((at, wide, *position));
            }
            at = at.saturating_add(wide);
        }
        found
    }

    /// A press on a position of the idle control; ignored while a rewrite is on its way.
    pub(crate) fn pick(&mut self, slot: Option<RewriteLength>) {
        if self.available && !matches!(self.phase, Phase::Rewriting { .. }) {
            self.position = slot;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Effect, Event, Rewrite};
    use yi_types::config::RewriteLength;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn text(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn enter_asks_then_enter_sends_the_proposal() {
        let mut control = Rewrite::ready(RewriteLength::Medium);
        assert_eq!(
            control.step(Event::Enter, "fix the tset", true),
            Effect::Ask
        );
        assert_eq!(control.asking(), Some((1, RewriteLength::Medium)));
        assert_eq!(
            control.step(Event::Enter, "fix the tset", true),
            Effect::Hold
        );
        assert_eq!(
            control.asking(),
            Some((1, RewriteLength::Medium)),
            "one call"
        );
        let shown = control.answer(1, Ok("Fix the test.".to_owned()), "fix the tset");
        assert_eq!(shown, Some(Ok("Fix the test.".to_owned())));
        assert_eq!(
            control.step(Event::Enter, "Fix the test.", true),
            Effect::Send
        );
        assert_eq!(control.step(Event::Enter, "next", true), Effect::Ask);
        assert_eq!(control.asking(), Some((2, RewriteLength::Medium)));
    }

    #[test]
    fn escape_restores_the_typed_words_and_they_go_as_typed_until_edited() {
        let mut control = Rewrite::ready(RewriteLength::Short);
        assert_eq!(control.step(Event::Escape, "typed", true), Effect::Pass);
        assert_eq!(control.step(Event::Enter, "typed", true), Effect::Ask);
        control.answer(1, Ok("Rewritten.".to_owned()), "typed");
        assert_eq!(
            control.step(Event::Escape, "Rewritten.", true),
            Effect::Restore
        );
        assert_eq!(control.typed(), Some("typed"));
        assert_eq!(control.step(Event::Escape, "typed", true), Effect::Pass);
        assert_eq!(control.step(Event::Enter, "typed", true), Effect::Send);

        assert_eq!(control.step(Event::Enter, "typed", true), Effect::Ask);
        assert_eq!(control.step(Event::Escape, "typed", true), Effect::Restore);
        assert_eq!(
            control.step(Event::Enter, "typed, edited", true),
            Effect::Ask
        );
    }

    #[test]
    fn a_failed_rewrite_leaves_the_draft_to_go_as_typed() {
        let mut control = Rewrite::ready(RewriteLength::Long);
        assert_eq!(control.step(Event::Enter, "typed", true), Effect::Ask);
        let told = control.answer(1, Err("no rewrite in 20 s".to_owned()), "typed");
        assert_eq!(told, Some(Err("no rewrite in 20 s".to_owned())));
        assert!(!control.locked("typed"));
        assert_eq!(control.step(Event::Enter, "typed", true), Effect::Send);
    }

    #[test]
    fn an_answer_to_a_box_that_moved_on_is_dropped() {
        let mut control = Rewrite::ready(RewriteLength::Medium);
        control.step(Event::Enter, "first", true);
        assert_eq!(
            control.answer(1, Ok("late".to_owned()), ""),
            None,
            "cleared"
        );
        control.step(Event::Enter, "second", true);
        assert_eq!(control.answer(1, Ok("late".to_owned()), "second"), None);
        assert!(control.locked("second"), "the second call is still open");
        assert!(!control.clone().locked(""), "a cleared box frees the keys");
        assert!(
            control
                .answer(2, Ok("on time".to_owned()), "second")
                .is_some()
        );
    }

    #[test]
    fn off_unavailable_and_ineligible_drafts_send_as_typed() {
        let mut absent = Rewrite::default();
        assert_eq!(absent.step(Event::Enter, "typed", true), Effect::Send);
        absent.cycle();
        assert_eq!(absent.step(Event::Enter, "typed", true), Effect::Send);
        let theme = crate::colors::Theme::new(crate::colors::ColorTier::Ansi16, true);
        assert!(absent.marker(&theme, 80, "typed").is_none());

        let mut control = Rewrite::ready(RewriteLength::Long);
        assert_eq!(control.step(Event::Enter, "/quit", false), Effect::Send);
        control.cycle();
        assert_eq!(
            control.step(Event::Enter, "typed", true),
            Effect::Send,
            "off"
        );
    }

    #[test]
    fn the_key_walks_off_short_medium_long_and_the_marker_names_the_position() {
        let theme = crate::colors::Theme::new(crate::colors::ColorTier::Ansi16, true);
        let mut control = Rewrite::ready(RewriteLength::Medium);
        let shown = |control: &mut Rewrite, width, held: &str| {
            (control.marker(&theme, width, held)).map(|line| text(&line))
        };
        let wide = Some(" improve: off · short · [medium] · long ".to_owned());
        assert_eq!(shown(&mut control, 80, ""), wide);
        assert_eq!(
            shown(&mut control, 51, ""),
            Some(" improve: medium ".to_owned())
        );
        let mut seen = Vec::new();
        for _ in 0..4 {
            control.cycle();
            seen.push(shown(&mut control, 40, "").unwrap_or_default());
        }
        let names = [
            " improve: long ",
            " improve: off ",
            " improve: short ",
            " improve: medium ",
        ];
        assert_eq!(seen, names);

        control.step(Event::Enter, "typed", true);
        control.cycle();
        assert_eq!(control.asking(), Some((1, RewriteLength::Medium)), "locked");
        let working = Some(" improving… · esc keeps your words ".to_owned());
        assert_eq!(shown(&mut control, 80, "typed"), working);
        control.answer(1, Ok("Rewritten.".to_owned()), "typed");
        let proposed = Some(" improved · enter sends · esc restores ".to_owned());
        assert_eq!(shown(&mut control, 80, "Rewritten."), proposed);
        assert_eq!(
            shown(&mut control, 80, ""),
            wide,
            "an emptied box is a new draft"
        );
    }

    #[test]
    fn each_position_rectangle_covers_its_label_in_the_painted_border() -> TestResult {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::Style;
        let theme = crate::colors::Theme::new(crate::colors::ColorTier::Ansi16, true);
        let mut composer = crate::composer::Composer::default();
        composer.rewrite = Rewrite::ready(RewriteLength::Medium);
        let foot = composer.rewrite.marker(&theme, 100, "");
        composer.set_foot(foot);
        composer.set_frame(Style::default(), Style::default(), Style::default(), None);
        let height = composer.layout(100, 30);
        let rect = Rect::new(0, 0, 100, height);
        let mut buffer = Buffer::empty(rect);
        composer.render(rect, &mut buffer);

        let (left, row) = composer.foot_at().ok_or("the paint left no foot")?;
        let under = |at: u16, wide: u16| -> String {
            (0..wide)
                .filter_map(|dx| buffer.cell((left + at + dx, row)))
                .map(|cell| cell.symbol())
                .collect()
        };
        let slots = composer.rewrite.slots(100);
        let seen: Vec<String> = slots
            .iter()
            .map(|(at, wide, _)| under(*at, *wide))
            .collect();
        assert_eq!(seen, ["off", "short", "[medium]", "long"]);
        let picks: Vec<_> = slots.iter().map(|slot| slot.2).collect();
        assert_eq!(picks[2], Some(RewriteLength::Medium));

        composer.rewrite.pick(Some(RewriteLength::Long));
        let moved = composer.rewrite.slots(100);
        assert_eq!(moved.len(), 4, "a pick keeps every position pickable");
        let narrow = composer.rewrite.slots(51);
        assert!(narrow.is_empty(), "the narrow form is not pickable");
        composer.rewrite.step(Event::Enter, "typed", true);
        assert!(
            composer.rewrite.slots(100).is_empty(),
            "nor is a working one"
        );
        Ok(())
    }
}
