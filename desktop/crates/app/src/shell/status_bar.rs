//! The status bar: branch, mode, model and effort, context, cost and turn on the left; what
//! needs you, what works, live agents and the server on the right.

use std::collections::BTreeSet;

use rune::{El, Icon, TextStyle, el, row, text};
use yd_core::ids::ThreadId;
use yd_core::model::Model;
use yd_core::reduce::state::{Link, Wire};
use yd_core::reduce::threads::{Alert, Thread, Want};
use yd_core::row::Dot;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::kit::icons::glyph;
use yd_ui::kit::status::thread_dot;
use yd_ui::line_icon;
use yd_ui::theme::look::{META, colour};
use yd_ui::theme::scale::LINE_BAR;
use yd_ui::win::{Hub, Win};

use crate::shell::view::{current, project, repo, roots};

pub const HEIGHT: f32 = 17.0;

static BRANCH: Icon = line_icon!(
    "branch",
    r#"<circle cx="7" cy="6" r="2"/><circle cx="7" cy="18" r="2"/><circle cx="17" cy="8" r="2"/><path d="M7 8v8M17 10c0 4-10 2-10 6"/>"#
);

const NUMBERS: TextStyle = META.features("'tnum' 1");

fn top_level(wire: &Wire) -> BTreeSet<&ThreadId> {
    roots(wire).into_iter().map(|thread| &thread.id).collect()
}

/// What the bell counts: one alert per top-level thread; a fork's alert shows on its root.
pub fn alerts(wire: &Wire) -> Vec<Alert> {
    let tops = top_level(wire);
    let mut seen = BTreeSet::new();
    let attention = wire.attention();
    attention
        .alerts
        .into_iter()
        .filter(|alert| tops.contains(&alert.thread) && seen.insert(alert.thread.clone()))
        .collect()
}

fn needs_you(wire: &Wire) -> usize {
    let waiting = |alert: &Alert| {
        matches!(
            alert.want,
            Want::Approval(_) | Want::NeedsYou | Want::Child(_)
        )
    };
    alerts(wire).iter().filter(|alert| waiting(alert)).count()
}

fn working(wire: &Wire) -> usize {
    let tops = top_level(wire);
    let attention = wire.attention();
    attention
        .working
        .iter()
        .filter(|id| tops.contains(id))
        .count()
}

/// Child agents of `thread` still running.
pub fn running(thread: &Thread) -> usize {
    let live = |status: &str| status == "running";
    thread
        .agents
        .iter()
        .filter(|agent| live(agent.update.status.as_str()))
        .count()
}

/// `turn 2 · 14m`: the thread's last turn and how long since it began.
pub fn turn(model: &Model, thread: &Thread) -> Option<String> {
    let last = thread.transcript.turns().pop()?;
    let minutes = model.now.0.saturating_sub(last.started.0) / 60_000;
    let elapsed = if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h", minutes / 60)
    };
    Some(format!("turn {} · {elapsed}", last.number))
}

/// `41.2K`, `200K`, `812`.
fn tokens_text(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    let tenths = count.saturating_add(50) / 100;
    match tenths % 10 {
        0 => format!("{}K", tenths / 10),
        tenth => format!("{}.{tenth}K", tenths / 10),
    }
}

/// The name of an option's current value: `Supervised`, `Opus 5.5`, `High`.
fn chosen(thread: &Thread, id: &str) -> Option<String> {
    let option = thread.option(id)?;
    let named = option
        .options
        .iter()
        .find(|choice| choice.value == option.current_value);
    Some(named.map_or_else(
        || option.current_value.clone(),
        |choice| choice.name.clone(),
    ))
}

fn words(words: impl Into<String>, ink: Rgba) -> El {
    text(words.into()).font(META).color(colour(ink))
}

fn item(key: &str) -> El {
    row()
        .key(["shell.status.", key].concat())
        .gap(4.0)
        .center_y()
}

fn left(hub: &Hub, thread: Option<&Thread>, tokens: &Tokens) -> Vec<El> {
    let model = &hub.model;
    let mut out = Vec::new();
    let place = thread.map_or("", |thread| project(&thread.cwd));
    let repo = repo(model, place);
    if let Some((repo, branch)) = repo.and_then(|repo| Some((repo, repo.words("branch")?))) {
        let mut facts = Vec::new();
        if let (Some(ahead), Some(behind)) = (repo.words("ahead"), repo.words("behind")) {
            facts.push(format!("↑{ahead} ↓{behind}"));
        }
        facts.extend(repo.words("commit").map(str::to_owned));
        out.push(
            item("branch")
                .child(glyph(&BRANCH, 10.0, tokens.dim))
                .child(words(branch, tokens.dim))
                .child(words(facts.join(" · "), tokens.faint)),
        );
    }
    let Some(thread) = thread else { return out };
    if let Some(mode) = chosen(thread, "mode") {
        out.push(item("mode").child(words(mode, tokens.dim)));
    }
    let brain = [chosen(thread, "model"), chosen(thread, "thought_level")];
    let brain: Vec<String> = brain.into_iter().flatten().collect();
    if !brain.is_empty() {
        out.push(item("model").child(words(brain.join(" · "), tokens.dim)));
    }
    let usage = &thread.usage;
    if thread.unreported() {
        out.push(item("context").child(words("no usage reported", tokens.faint)));
        out.extend(turn(model, thread).map(|turn| item("turn").child(words(turn, tokens.faint))));
        return out;
    }
    let half = usage.window.0 / 2;
    let hundredths = usage.used.0.saturating_mul(100).saturating_add(half);
    if let Some(share) = hundredths.checked_div(usage.window.0) {
        let held = format!(
            "{} of {}",
            tokens_text(usage.used.0),
            tokens_text(usage.window.0)
        );
        out.push(
            item("context")
                .child(text(held).font(NUMBERS).color(colour(tokens.dim)))
                .child(
                    text(format!("{share}%"))
                        .font(NUMBERS)
                        .color(colour(tokens.faint)),
                ),
        );
    }
    if usage.ledger.is_some() || thread.transcript.since.replies > 0 {
        let cost = format!("${:.2}", thread.cost());
        out.push(item("cost").child(text(cost).font(NUMBERS).color(colour(tokens.dim))));
    }
    if let Some(turn) = turn(model, thread) {
        out.push(item("turn").child(words(turn, tokens.faint)));
    }
    out
}

fn right(hub: &Hub, thread: Option<&Thread>, tokens: &Tokens) -> Vec<El> {
    let wire = &hub.model.wire;
    let mut out = Vec::new();
    let need = needs_you(wire);
    if need > 0 {
        out.push(
            item("need")
                .child(thread_dot(tokens, Dot::Need, 6.0, false))
                .child(words(format!("{need} need you"), tokens.dim)),
        );
    }
    let busy = working(wire);
    if busy > 0 {
        out.push(
            item("working")
                .child(thread_dot(tokens, Dot::Work, 6.0, false))
                .child(words(format!("{busy} working"), tokens.dim)),
        );
    }
    let agents = thread.map_or(0, running);
    if agents > 0 {
        let noun = if agents == 1 { "agent" } else { "agents" };
        out.push(item("agents").child(words(format!("{agents} {noun}"), tokens.faint)));
    }
    // Up, the link says how long the server took to answer; a replay timed nothing.
    let link = match &wire.link {
        Link::Up { latency, .. } => latency.map(|took| format!("{} ms", took.0)),
        Link::Starting => Some("starting".to_owned()),
        Link::Down { .. } => Some("offline".to_owned()),
    };
    out.push(
        item("server")
            .child(words("yi serve", tokens.dim))
            .children(link.map(|state| words(state, tokens.faint))),
    );
    out
}

pub fn view(win: &Win, hub: &Hub) -> El {
    let tokens = &hub.model.tokens;
    let thread = current(hub, win.window);
    row()
        .key("shell.status-bar")
        .w_full()
        .h(HEIGHT)
        .px(8.0)
        .gap(9.0)
        .center_y()
        .clip_rounded(0.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_BAR.of(tokens)))
        .children(left(hub, thread, tokens))
        .child(el().grow(1.0).min_w(0.0))
        .children(right(hub, thread, tokens))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::ids::Millis;
    use yd_core::msg::{Msg, step};
    use yd_core::wire::event::WireEvent;
    use yd_core::wire::replay::play;

    use super::*;

    /// Dogfood: "turn 1 · 0m" after a 3 m 38 s turn. The running app never ticks the model's
    /// clock; the time comes with the wire's events, as here.
    #[test]
    fn the_turn_reads_its_minutes_from_the_wires_own_time() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        let began = Millis(1_794_679_124_000);
        step(&mut model, Msg::Wire(WireEvent::Poll { at: began }));
        let recording = include_str!("../../../../fixtures/wire/approval-allow.jsonl");
        play(&mut model, recording)?;
        let later = Millis(began.0 + 218_000);
        step(&mut model, Msg::Wire(WireEvent::Poll { at: later }));
        let thread = model.wire.listed().into_iter().next().ok_or("no thread")?;
        assert_eq!(turn(&model, thread).as_deref(), Some("turn 1 · 3m"));
        Ok(())
    }
}
