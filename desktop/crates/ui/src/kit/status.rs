//! Status dot, tag, severity badge, state pill and colour swatch (record 1.1.3, 7.2).

use rune::{El, Icon, LoopSpec, Shadow, TextStyle, el, icon, row, text};
use yd_core::row::{Dot, Tone};
use yd_core::tokens::{Rgba, Tokens};

use crate::theme::look::{META, colour, tone};
use crate::theme::scale::{BADGE_FILL, PILL_FIXING, RADIUS_BADGE, RING_SWATCH, SEMIBOLD, TAG_FILL};

/// The working dot: an arc open on the right, turned 30 degrees. It spins; under reduced motion
/// Rune holds a rotation at its start, so it stands still.
static ARC: Icon = Icon {
    name: "status-work",
    view: 8.0,
    body: r#"<path d="M4.776 6.898A3 3 0 1 1 6.898 3.224" fill="none" stroke="currentColor" stroke-width="2"/>"#,
};

/// One turn of the working ring, in seconds.
const SPIN: f64 = 1.2;

/// What a dot of each state is drawn in. Done is green: the role rule wins over the render.
pub fn dot_colour(tokens: &Tokens, state: Dot) -> Rgba {
    match state {
        Dot::Need | Dot::Ask | Dot::Pause => tokens.need,
        Dot::Work => tokens.accent,
        Dot::Done | Dot::Run => tokens.ok,
        Dot::Idle => tokens.idle,
        Dot::Stop => tokens.stopped,
        Dot::Fail => tokens.bad,
    }
}

fn circle(size: f32, fill: Rgba) -> El {
    el().size(size, size).radius(size / 2.0).fill(colour(fill))
}

fn ring(size: f32, ink: Rgba, width: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .border(width, colour(ink))
}

/// An item's 7 px dot: filled in its state's colour; asking is a hollow ring.
pub fn dot(tokens: &Tokens, state: Dot) -> El {
    let ink = dot_colour(tokens, state);
    match state {
        Dot::Ask => ring(7.0, ink, 1.5),
        Dot::Need
        | Dot::Work
        | Dot::Done
        | Dot::Idle
        | Dot::Run
        | Dot::Stop
        | Dot::Fail
        | Dot::Pause => circle(7.0, ink),
    }
}

/// A thread's dot at `size` (9, 8, 7 or 6): needs-you carries a 3 px halo unless `halo` is off,
/// asking is a ring, working the still arc, idle draws nothing.
pub fn thread_dot(tokens: &Tokens, state: Dot, size: f32, halo: bool) -> El {
    let ink = dot_colour(tokens, state);
    match state {
        Dot::Need if halo => circle(size, ink).box_shadow(Shadow {
            spread: 3.0,
            color: colour(ink.alpha(0.26)),
            ..Shadow::default()
        }),
        Dot::Ask => ring(size, ink, 2.0),
        Dot::Work => icon(&ARC, size)
            .color(colour(ink))
            .looping(LoopSpec::rotation(SPIN)),
        Dot::Idle => el().size(size, size),
        Dot::Need | Dot::Done | Dot::Run | Dot::Stop | Dot::Fail | Dot::Pause => circle(size, ink),
    }
}

/// Tags and pills: 9.5 with their own line heights.
pub const TAG: TextStyle = META.lh(13.0);
pub const PILL: TextStyle = META.lh(14.0);
pub const BADGE: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..META
}
.lh(13.0)
.tracking(0.19);

/// A small tag; `warn` is the needs-you variant ("needs sign-in").
pub fn tag(tokens: &Tokens, words: &str, warn: bool) -> El {
    let (fill, ink) = if warn {
        (tokens.need.alpha(0.18), tokens.need)
    } else {
        (TAG_FILL.of(tokens), tokens.dim)
    };
    row()
        .h(13.0)
        .px(4.0)
        .center_y()
        .radius(RADIUS_BADGE.of(tokens))
        .fill(colour(fill))
        .child(text(words).font(TAG).color(colour(ink)))
}

/// A severity badge: `Bad` High, `Warn` Med, anything else Low.
pub fn badge(tokens: &Tokens, words: &str, level: Tone) -> El {
    let (fill, ink) = match level {
        Tone::Bad => (tokens.bad.alpha(0.22), tokens.bad),
        Tone::Warn => (tokens.warn.alpha(0.16), tokens.warn),
        Tone::Fg
        | Tone::Dim
        | Tone::Faint
        | Tone::Accent
        | Tone::Need
        | Tone::Ok
        | Tone::Model
        | Tone::Context => (BADGE_FILL.of(tokens), tokens.dim),
    };
    row()
        .h(13.0)
        .min_w(32.0)
        .px(5.0)
        .center()
        .radius(RADIUS_BADGE.of(tokens))
        .fill(colour(fill))
        .child(text(words).font(BADGE).color(colour(ink)))
}

/// A fully round state pill: New is `Context`, Fixing `Accent`, Proving `Need`, Watching and
/// Done `Ok`, Open anything else.
pub fn pill(tokens: &Tokens, words: &str, state: Tone) -> El {
    let (fill, ink) = match state {
        Tone::Context => (tokens.context.alpha(0.2), tokens.context),
        Tone::Accent => (tokens.accent.alpha(0.2), PILL_FIXING.of(tokens)),
        Tone::Need => (tokens.need.alpha(0.2), tokens.need),
        Tone::Ok => (tokens.ok.alpha(0.14), tokens.ok),
        Tone::Fg | Tone::Dim | Tone::Faint | Tone::Bad | Tone::Warn | Tone::Model => {
            (BADGE_FILL.of(tokens), tokens.dim)
        }
    };
    row()
        .h(14.0)
        .px(6.0)
        .center_y()
        .radius(7.0)
        .fill(colour(fill))
        .child(text(words).font(PILL).color(colour(ink)))
}

/// A 12 px colour swatch in a light ring.
pub fn swatch(tokens: &Tokens, role: Tone) -> El {
    el().size(12.0, 12.0)
        .radius(RADIUS_BADGE.of(tokens))
        .fill(colour(tone(tokens, role)))
        .box_shadow(Shadow {
            spread: 1.0,
            color: colour(RING_SWATCH.of(tokens)),
            ..Shadow::default()
        })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Env, Runtime, col};

    use super::*;

    #[test]
    fn the_working_ring_turns() -> Result<(), Box<dyn Error>> {
        let view = |_: &(), _: &mut rune::Cx<()>| {
            col().child(thread_dot(&Tokens::default(), Dot::Work, 9.0, true))
        };
        let mut rt = Runtime::new((), view, (40.0, 40.0), Env::new(true, 1.0));
        for _ in 0..4 {
            rt.frame(0.1);
        }
        let phases: Vec<f64> = rt.elements().iter().filter_map(|e| e.loop_phase).collect();
        assert!(
            phases.iter().any(|phase| *phase > 0.0),
            "it stands still: {phases:?}"
        );
        Ok(())
    }
}
