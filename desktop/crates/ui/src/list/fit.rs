//! Which columns a list has room for: a narrow pane drops whole columns, least important first.

use yd_core::list::{ListDesc, RowHeight, Text, Width};

use super::row::GAP;

/// The least the flexible column keeps, in points.
const LEAST: f32 = 80.0;
/// The least a flexible column of faint asides keeps.
const FAINT: f32 = 40.0;

/// Every column of `desc`, one bit each.
pub fn all(desc: &ListDesc) -> u32 {
    match desc.columns.len() {
        0 => 0,
        n if n >= 32 => u32::MAX,
        n => (1_u32 << n) - 1,
    }
}

/// Whether the column at `at` is kept in `mask`.
pub fn has(mask: u32, at: usize) -> bool {
    u32::try_from(at).is_ok_and(|at| mask.checked_shr(at).is_some_and(|bits| bits & 1 == 1))
}

/// How soon a column goes: faint words first, then figures, then mono, then the rest.
fn rank(text: Text) -> u8 {
    match text {
        Text::Faint => 0,
        Text::Number => 1,
        Text::Mono => 2,
        Text::Plain | Text::Mark => 3,
        Text::Name => u8::MAX,
    }
}

/// The columns kept in `room` points: the name always stays, a log's flexible column keeps
/// half the room, and the rest go least important first; a flexible column of words goes last.
pub fn kept(desc: &ListDesc, room: f32) -> u32 {
    let mut mask = all(desc);
    let named = desc.columns.iter().any(|column| column.text == Text::Name);
    let share = if desc.height == RowHeight::Line {
        0.5
    } else {
        0.3
    };
    loop {
        let mut flexible = desc
            .columns
            .iter()
            .enumerate()
            .filter(|(at, column)| has(mask, *at) && matches!(column.width, Width::Fr(_)));
        // A flexible column of faint asides, such as a type, is worth less room than words.
        let least = match flexible.next() {
            Some((_, column)) if column.text == Text::Faint => FAINT,
            Some(_) => (room * share).max(LEAST),
            None => 0.0,
        };
        let shown = desc
            .columns
            .iter()
            .enumerate()
            .filter(|(at, _)| has(mask, *at));
        let (fixed, count) = shown.fold(
            (0.0_f32, 0_u32),
            |(fixed, count), (_, column)| match column.width {
                Width::Px(points) => (fixed + points, count + 1),
                Width::Fr(_) => (fixed, count + 1),
            },
        );
        let gaps = GAP * count.saturating_sub(1) as f32;
        if fixed + gaps + least <= room {
            return mask;
        }
        let next = desc
            .columns
            .iter()
            .enumerate()
            .filter(|(at, column)| has(mask, *at) && column.text != Text::Name)
            // A list with a name keeps the name alone before it keeps a squeezed column.
            .filter(|(_, column)| {
                named || column.text == Text::Faint || matches!(column.width, Width::Px(_))
            })
            .min_by_key(|(at, column)| {
                let late = matches!(column.width, Width::Fr(_)) && column.text != Text::Faint;
                (late, rank(column.text), std::cmp::Reverse(*at))
            });
        let Some((at, _)) = next else {
            return mask;
        };
        mask &= !(1_u32 << at);
    }
}

#[cfg(test)]
mod tests {
    use yd_core::kind::Kind;
    use yd_core::list::Col;

    use super::*;

    const fn col(field: &'static str, width: Width, text: Text) -> Col {
        Col {
            field,
            title: "",
            width,
            text,
        }
    }

    const fn list(columns: &'static [Col], height: RowHeight) -> ListDesc {
        ListDesc {
            kind: Kind::LogLine,
            columns,
            tree: false,
            height,
            facts_per_line: 1,
            detail: |_| yd_core::list::Detail::default(),
            footer: None,
        }
    }

    const LOGS: ListDesc = list(
        &[
            col("time", Width::Px(80.0), Text::Faint),
            col("level", Width::Px(36.0), Text::Plain),
            col("source", Width::Px(64.0), Text::Faint),
            col("message", Width::Fr(1.0), Text::Plain),
        ],
        RowHeight::Line,
    );

    const AGENTS: ListDesc = list(
        &[
            col("state", Width::Px(7.0), Text::Plain),
            col("agent", Width::Px(131.0), Text::Name),
            col("doing", Width::Fr(1.0), Text::Plain),
            col("model", Width::Px(12.0), Text::Mono),
            col("tokens", Width::Px(38.0), Text::Number),
            col("cost", Width::Px(36.0), Text::Number),
            col("time", Width::Px(30.0), Text::Number),
            col("last", Width::Px(60.0), Text::Plain),
        ],
        RowHeight::Row,
    );

    fn names(desc: &ListDesc, mask: u32) -> Vec<&'static str> {
        let shown = desc
            .columns
            .iter()
            .enumerate()
            .filter(|(at, _)| has(mask, *at));
        shown.map(|(_, column)| column.field).collect()
    }

    #[test]
    fn a_wide_list_keeps_every_column() {
        assert_eq!(kept(&AGENTS, 700.0), all(&AGENTS));
        assert_eq!(kept(&LOGS, 700.0), all(&LOGS));
    }

    #[test]
    fn a_narrow_log_drops_its_faint_columns_and_keeps_half_the_room_for_the_message() {
        let mask = kept(&LOGS, 254.0);
        assert_eq!(names(&LOGS, mask), ["level", "message"]);
    }

    #[test]
    fn narrow_agents_drop_figures_before_the_state_and_never_the_name() {
        let mask = kept(&AGENTS, 254.0);
        assert_eq!(names(&AGENTS, mask), ["state", "agent", "doing"]);
        let mask = kept(&AGENTS, 144.0);
        assert_eq!(names(&AGENTS, mask), ["agent"]);
        let mask = kept(&AGENTS, 10.0);
        assert!(names(&AGENTS, mask).contains(&"agent"));
    }
}
