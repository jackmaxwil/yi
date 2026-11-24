//! The Ready gate: what blocks calling the thread done.

use rune::{El, col, row, text};
use yd_core::findings::state::Mark;
use yd_core::findings::view::Ready;
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::footer;
use yd_ui::theme::look::{LABEL, META, colour};

use crate::findings::hub::detail::check;
use crate::findings::hub::title;

const HEADLINE: rune::TextStyle = rune::TextStyle::new(15.0, 650.0).lh(18.0);

/// `1 open`, `2 waiting`: a count, else the words for none.
fn many(n: usize, some: &str, none: &str) -> String {
    if n == 0 {
        none.to_owned()
    } else {
        format!("{n} {some}")
    }
}

/// The gate's checks, from the model's [`Ready`]: High findings, proofs, triage left.
pub fn checks(gate: &Ready) -> [(&'static str, Mark, String); 3] {
    let high = gate.high.len();
    let unproven = gate.unproven.len();
    let left = usize::try_from(gate.untriaged).unwrap_or(usize::MAX);
    [
        (
            "No High findings open",
            if high == 0 { Mark::Met } else { Mark::Failing },
            many(high, "open", "none open"),
        ),
        (
            "Every fix is proven",
            if unproven == 0 {
                Mark::Met
            } else {
                Mark::Waiting
            },
            many(unproven, "waiting", "all proven"),
        ),
        // Invariant: triage never blocks, so this check is met however many are left.
        (
            "New findings triaged",
            Mark::Met,
            if left == 0 {
                "all triaged".to_owned()
            } else {
                format!("{left} left, not blocking")
            },
        ),
    ]
}

/// Whether the work may be called done, and what stands in the way (record 6.6.3).
pub fn view(tokens: &Tokens, key: &str, gate: &Ready) -> El {
    let blockers = gate.blockers();
    let (said, ink, why) = if gate.ready() {
        ("Ready", tokens.ok, "nothing stands in the way".to_owned())
    } else {
        (
            "Not yet",
            tokens.need,
            many(blockers, "things stand in the way", ""),
        )
    };
    let why = if blockers == 1 {
        "1 thing stands in the way".to_owned()
    } else {
        why
    };
    let meta = match blockers {
        0 => "no blockers".to_owned(),
        1 => "1 blocker".to_owned(),
        n => format!("{n} blockers"),
    };
    let rows = checks(gate)
        .into_iter()
        .enumerate()
        .map(|(n, (words, mark, note))| {
            check(tokens, format!("{key}.gate.check.{n}"), words, mark, &note)
        });
    col()
        .key(format!("{key}.gate"))
        .w_full()
        .child(title(tokens, &format!("{key}.gate-title"), "Ready?", &meta))
        .child(
            row()
                .key(format!("{key}.gate.verdict"))
                .px(8.0)
                .pt(6.0)
                .gap(8.0)
                .items_end()
                .child(text(said).font(HEADLINE).color(colour(ink)))
                .child(text(why).font(LABEL).color(colour(tokens.dim))),
        )
        .child(col().w_full().px(8.0).py(4.0).children(rows))
        .child(
            footer(
                tokens,
                vec![
                    text("Agents are held to this too")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.gate.footer")),
        )
}
