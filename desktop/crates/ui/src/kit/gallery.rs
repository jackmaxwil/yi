//! The gallery: every shared component in each of its states, in one scene.

use rune::{Cx, El, col, row, text};
use yd_core::row::{Dot, Tone};
use yd_core::tokens::Tokens;

use crate::kit::bars::{footer, header, lead};
use crate::kit::button::{Button, button};
use crate::kit::chip::{Chip, chip};
use crate::kit::icons::SEARCH;
use crate::kit::key_hint::key_hint;
use crate::kit::label::{key_values, section};
use crate::kit::meter::{bar, meter};
use crate::kit::segmented::segmented;
use crate::kit::select::select;
use crate::kit::slider::slider;
use crate::kit::sparkline::sparkline;
use crate::kit::status::{badge, dot, pill, swatch, tag, thread_dot};
use crate::kit::toggle::toggle;
use crate::kit::tooltip::hover_card;
use crate::pane::{Scene, Stage};
use crate::theme::look::{
    BODY, CODE_LINE, LABEL, META, READ_PROSE, READ_TITLE, TITLE, colour, white,
};
use crate::win::{Hub, Win};

pub const SCENES: &[Scene] = &[Scene {
    name: "kit/gallery",
    stage: Stage::View(view),
    setup: |_| {},
}];

const DOTS: [(&str, Dot); 9] = [
    ("need", Dot::Need),
    ("ask", Dot::Ask),
    ("work", Dot::Work),
    ("done", Dot::Done),
    ("idle", Dot::Idle),
    ("run", Dot::Run),
    ("stop", Dot::Stop),
    ("fail", Dot::Fail),
    ("pause", Dot::Pause),
];

fn group(tokens: &Tokens, title: &str, samples: Vec<El>) -> El {
    col().child(section(tokens, title, None, None)).child(
        row()
            .px(tokens.gutter)
            .gap(8.0)
            .center_y()
            .children(samples),
    )
}

/// The gallery's controls press in and spring back but change nothing.
fn still(_: &mut Win) {}

fn faint(tokens: &Tokens, words: &str) -> El {
    text(words).font(META).color(colour(tokens.faint))
}

fn controls(tokens: &Tokens) -> Vec<El> {
    let pick = |_: &mut Win, _: usize| {};
    vec![
        group(
            tokens,
            "Buttons",
            vec![
                button(tokens, Button::Plain, "Edit", None)
                    .key("kit.button.plain")
                    .on_press(still),
                button(tokens, Button::Primary, "Run", Some("↵"))
                    .key("kit.button.primary")
                    .on_press(still),
                button(tokens, Button::Ghost, "Deny", None)
                    .key("kit.button.ghost")
                    .on_press(still),
                button(tokens, Button::Plain, "Always", Some("⌘↵"))
                    .key("kit.button.plain-hint")
                    .on_press(still),
            ],
        ),
        group(
            tokens,
            "Chips",
            vec![
                chip(tokens, Chip::Plain, "All", None, false)
                    .key("kit.chip.plain")
                    .on_press(still),
                chip(tokens, Chip::Pill, "Touched", None, false)
                    .key("kit.chip.pill")
                    .on_press(still),
                chip(tokens, Chip::Plain, "Filter", Some(&SEARCH), false)
                    .key("kit.chip.icon")
                    .on_press(still),
                chip(tokens, Chip::Plain, "This thread", None, true)
                    .key("kit.chip.menu")
                    .on_press(still),
            ],
        ),
        group(
            tokens,
            "Segmented, toggle, select",
            vec![
                segmented(tokens, "kit.segmented.first", &["Tree", "Graph"], 0, pick),
                segmented(
                    tokens,
                    "kit.segmented.last",
                    &["Dense", "Compact", "Roomy"],
                    2,
                    pick,
                ),
                toggle(tokens, "kit.toggle.off", false),
                toggle(tokens, "kit.toggle.on", true),
                select(tokens, "Yi Dark").key("kit.select").on_press(still),
            ],
        ),
        group(
            tokens,
            "Slider and keys",
            vec![
                slider(tokens, "kit.slider", 0.6, "60%", |_: &mut Win, _| {}),
                key_hint(tokens, "⌘K").key("kit.hint"),
                key_hint(tokens, "esc"),
                key_hint(tokens, "↵"),
            ],
        ),
        group(
            tokens,
            "Item dots",
            DOTS.iter()
                .map(|(name, state)| {
                    row()
                        .gap(4.0)
                        .center_y()
                        .child(dot(tokens, *state).key(format!("kit.dot.{name}")))
                        .child(faint(tokens, name))
                })
                .collect(),
        ),
        group(
            tokens,
            "Thread dots",
            DOTS.iter()
                .take(5)
                .map(|(name, state)| {
                    thread_dot(tokens, *state, 8.0, true).key(format!("kit.thread-dot.{name}"))
                })
                .collect(),
        ),
        group(
            tokens,
            "Tags and badges",
            vec![
                tag(tokens, "upload.rs", false).key("kit.tag.plain"),
                tag(tokens, "needs sign-in", true).key("kit.tag.warn"),
                badge(tokens, "High", Tone::Bad).key("kit.badge.high"),
                badge(tokens, "Med", Tone::Warn).key("kit.badge.med"),
                badge(tokens, "Low", Tone::Dim).key("kit.badge.low"),
            ],
        ),
        group(
            tokens,
            "Pills",
            vec![
                pill(tokens, "New", Tone::Context).key("kit.pill.new"),
                pill(tokens, "Open", Tone::Dim).key("kit.pill.open"),
                pill(tokens, "Fixing", Tone::Accent).key("kit.pill.fixing"),
                pill(tokens, "Proving", Tone::Need).key("kit.pill.proving"),
                pill(tokens, "Watching", Tone::Ok).key("kit.pill.watching"),
            ],
        ),
        group(
            tokens,
            "Meters, bars, sparkline",
            vec![
                meter(tokens, "kit.meter.low", 0.41),
                meter(tokens, "kit.meter.warm", 0.7),
                meter(tokens, "kit.meter.hot", 0.9),
                bar(tokens, 60.0, 0.5, false).key("kit.bar"),
                bar(tokens, 60.0, 0.8, true).key("kit.bar.heavy"),
                sparkline(tokens, &[2.0, 5.0, 3.0, 8.0, 6.0, 9.0, 4.0]).key("kit.sparkline"),
            ],
        ),
    ]
}

fn surfaces(tokens: &Tokens) -> Vec<El> {
    let pane = col()
        .w(517.0)
        .h(150.0)
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .border_sides([0.0, 1.0, 1.0, 0.0], colour(white(tokens.lines)))
        .child(
            header(
                tokens,
                vec![
                    lead(tokens, "Rate limit"),
                    faint(tokens, "2 working · 2 need you"),
                ],
                vec![chip(tokens, Chip::Plain, "This thread", None, true)],
            )
            .key("kit.header"),
        )
        .child(section(tokens, "Agent", Some("6"), Some("depth 2")).key("kit.section"))
        .child(
            col()
                .px(tokens.gutter)
                .grow(1.0)
                .child(key_values(
                    tokens,
                    &[
                        ("Time", "17:58:03.412"),
                        ("Level", "ERROR"),
                        ("Source", "docker:db"),
                        ("Peer", "198.51.100.14:5432"),
                    ],
                ))
                .key("kit.key-values"),
        )
        .child(
            footer(
                tokens,
                vec![faint(tokens, "5 sources · 1,204 lines this turn")],
                vec![faint(tokens, "1 error · 3 warnings")],
            )
            .key("kit.footer"),
        );
    let card = hover_card(
        tokens,
        "pub fn take(&mut self, now: Instant) -> Result<(), Duration>",
        "Takes one token, refilling the bucket when its window has passed.",
        &["Go to definition", "Find references"],
    )
    .key("kit.hover-card");
    let sizes = col()
        .gap(4.0)
        .child(
            text("Finding and plugin titles")
                .font(TITLE)
                .color(colour(tokens.fg))
                .key("kit.type.title"),
        )
        .child(
            text("Chat, and the name in every row")
                .font(BODY)
                .color(colour(tokens.fg))
                .key("kit.type.body"),
        )
        .child(
            text("Tabs, values, buttons, code")
                .font(LABEL)
                .color(colour(tokens.dim))
                .key("kit.type.label"),
        )
        .child(
            text("Times, counts, section labels")
                .font(META)
                .color(colour(tokens.faint))
                .key("kit.type.meta"),
        )
        .child(
            text("let left = self.tokens;")
                .font(CODE_LINE)
                .color(colour(tokens.fg)),
        )
        .child(
            text("Reading size")
                .font(READ_TITLE)
                .color(colour(tokens.fg)),
        )
        .child(
            text("Full-screen chat reads larger.")
                .font(READ_PROSE)
                .color(colour(tokens.fg)),
        );
    let roles = [
        Tone::Accent,
        Tone::Need,
        Tone::Ok,
        Tone::Bad,
        Tone::Warn,
        Tone::Model,
        Tone::Context,
    ];
    vec![
        group(tokens, "Pane bars", vec![pane]),
        group(tokens, "Hover card", vec![card]),
        group(
            tokens,
            "Type and colour",
            vec![
                sizes,
                col()
                    .gap(4.0)
                    .children(roles.iter().map(|role| swatch(tokens, *role))),
            ],
        ),
    ]
}

/// Every component on the window's own fill, in two columns.
pub fn view(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    row()
        .key("kit")
        .w_full()
        .h_full()
        .padding(8.0)
        .gap(16.0)
        .child(col().w(560.0).gap(8.0).children(controls(tokens)))
        .child(col().gap(8.0).children(surfaces(tokens)))
}

#[cfg(test)]
mod tests;
