//! Chat steps coloured as what they are, read from the glyphs and fills the window paints:
//! a recorded turn of bash, read, grep, ipython and edit calls on Yi's own tools.

use rune::Color;
use rune::render::{KIND_GLYPH, KIND_RECT};
use yd_core::tokens::Tokens;
use yd_ui::theme::look::colour;

use super::*;

const TOOLS: &str = include_str!("../../../../../../fixtures/wire/tools-turn.jsonl");
const CHILD: &str = include_str!("../../../../../../fixtures/wire/child-turn.jsonl");

/// The main window over `recording`, the chat pane alone beside the sidebar.
fn window(recording: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    {
        let mut hub = win.hub.try_borrow_mut()?;
        yd_core::wire::replay::play(&mut hub.model, recording)?;
        root::blank(&mut hub, root::MAIN);
    }
    let mut runtime = Runtime::new(win, root::view, (1200.0, 900.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    Ok(runtime)
}

fn step(runtime: &Runtime<Win>, call: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let key = shown.iter().filter_map(|e| e.key.as_deref());
    let mut found = key.filter(|k| k.starts_with("chat-") && k.ends_with(&format!(".step.{call}")));
    Ok(found
        .next()
        .ok_or_else(|| format!("no step {call}"))?
        .to_owned())
}

/// Opens the step of `call` by a press on its verb.
fn open(runtime: &mut Runtime<Win>, call: &str) -> Result<String, Box<dyn Error>> {
    let key = step(runtime, call)?;
    tap(runtime, &format!("{key}.verb"))?;
    Ok(key)
}

fn tokens(runtime: &Runtime<Win>) -> Result<Tokens, Box<dyn Error>> {
    Ok(runtime.state.hub.try_borrow()?.model.tokens.clone())
}

/// The colours the window painted with `kind` inside the element keyed `key`.
fn painted(runtime: &Runtime<Win>, kind: f32, key: &str) -> Result<Vec<[f32; 4]>, Box<dyn Error>> {
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, key)
        .ok_or_else(|| format!("no {key}"))?
        .visible;
    let inside = |r: &[f32; 4]| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        cx >= x && cx <= x + w && cy >= y && cy <= y + h
    };
    let of = runtime.instances.iter().filter(|i| i.params[1] == kind);
    Ok(of.filter(|i| inside(&i.rect)).map(|i| i.fill).collect())
}

/// Whether one of `fills` is `c`, as drawn straight or with its alpha multiplied in.
fn holds(fills: &[[f32; 4]], c: Color) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() < 0.004;
    fills.iter().any(|f| {
        let straight = near(f[0], c.r) && near(f[1], c.g) && near(f[2], c.b);
        let premul = near(f[0], c.r * c.a) && near(f[1], c.g * c.a) && near(f[2], c.b * c.a);
        near(f[3], c.a) && (straight || premul)
    })
}

fn text_of(runtime: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let found = by_key(&shown, key).ok_or_else(|| format!("no {key}"))?;
    Ok(found.text.clone().unwrap_or_default())
}

#[test]
fn a_commands_ansi_colour_is_drawn_in_the_terminal_palette_and_never_printed_raw() -> Check {
    let mut runtime = window(TOOLS)?;
    let t = tokens(&runtime)?;
    let closed = step(&runtime, "t1")?;
    let command = painted(&runtime, KIND_GLYPH, &format!("{closed}.target"))?;
    assert!(
        holds(&command, colour(t.string)),
        "the closed row's command reads as shell"
    );
    let key = open(&mut runtime, "t1")?;
    let output = format!("{key}.output");
    let words = text_of(&runtime, &output)?;
    assert!(
        words.starts_with("9442ca6 Start tally\nred plain"),
        "{words:?}"
    );
    assert!(
        !words.contains('\u{1b}') && !words.contains("[31m"),
        "{words:?}"
    );
    let inks = painted(&runtime, KIND_GLYPH, &output)?;
    assert!(holds(&inks, colour(t.bad)), "red is the palette's red");
    assert!(
        holds(&inks, colour(t.dim)),
        "and the rest is the output's own colour"
    );
    Ok(())
}

#[test]
fn a_read_is_coloured_in_its_files_language_and_a_search_marks_its_hits() -> Check {
    let mut runtime = window(TOOLS)?;
    let t = tokens(&runtime)?;
    let read = open(&mut runtime, "t2")?;
    let inks = painted(&runtime, KIND_GLYPH, &format!("{read}.output"))?;
    for (ink, what) in [
        (t.keyword, "keyword"),
        (t.comment, "comment"),
        (t.faint, "number"),
    ] {
        assert!(holds(&inks, colour(ink)), "lib.rs draws its {what} colour");
    }
    let grep = open(&mut runtime, "t3")?;
    let inks = painted(&runtime, KIND_GLYPH, &format!("{grep}.output"))?;
    assert!(holds(&inks, colour(t.keyword)), "a hit is Rust too");
    let hit = |n: usize| runtime.find(&format!("{grep}.output.hit.{n}"));
    assert!(hit(0).is_some() && hit(1).is_some(), "both hits are marked");
    assert!(hit(2).is_none(), "and only the two");
    Ok(())
}

#[test]
fn an_ipython_cell_reads_as_python_and_only_a_traceback_of_its_output_is_tinted() -> Check {
    let mut runtime = window(TOOLS)?;
    let t = tokens(&runtime)?;
    let closed = step(&runtime, "t4")?;
    let first = text_of(&runtime, &format!("{closed}.target"))?;
    assert_eq!(first, "import json");
    let cell = open(&mut runtime, "t4")?;
    let code = painted(&runtime, KIND_GLYPH, &format!("{cell}.command"))?;
    assert!(holds(&code, colour(t.keyword)), "import is a keyword");
    let printed = painted(&runtime, KIND_GLYPH, &format!("{cell}.output"))?;
    assert!(
        printed.iter().all(|f| holds(&[*f], colour(t.dim))),
        "JSON and then a page of help is not one language: plain"
    );
    let failed = open(&mut runtime, "t5")?;
    let inks = painted(&runtime, KIND_GLYPH, &format!("{failed}.output"))?;
    for (ink, what) in [(t.string, "file"), (t.number, "line"), (t.bad, "error")] {
        assert!(
            holds(&inks, colour(ink)),
            "the traceback's {what} is tinted"
        );
    }
    Ok(())
}

#[test]
fn an_edit_shows_its_diff_with_the_added_line_tinted() -> Check {
    let mut runtime = window(TOOLS)?;
    let t = tokens(&runtime)?;
    let edit = open(&mut runtime, "t7")?;
    let output = format!("{edit}.output");
    assert!(text_of(&runtime, &output)?.contains("\n+Built with tally.\n"));
    let fills = painted(&runtime, KIND_RECT, &output)?;
    assert!(holds(&fills, colour(t.added)), "the added line is tinted");
    assert!(holds(
        &painted(&runtime, KIND_GLYPH, &output)?,
        colour(t.ok)
    ));
    let diff = open(&mut runtime, "t8")?;
    let fills = painted(&runtime, KIND_RECT, &format!("{diff}.output"))?;
    assert!(
        holds(&fills, colour(t.added)),
        "git diff's output is a diff"
    );
    Ok(())
}

/// The recording up to the event that names `call`'s ask, with the recorder's answer to that
/// ask left out, so the card waits on it.
fn waiting_on(call: &str) -> Result<String, Box<dyn Error>> {
    let named = format!("\"permission_requested\",\"toolCallId\":\"{call}\"");
    let lines: Vec<&str> = TOOLS.lines().collect();
    let last = lines
        .iter()
        .position(|l| l.contains(&named))
        .ok_or("no ask")?;
    let head = lines.get(..=last).ok_or("cut")?;
    let asked = head
        .iter()
        .rposition(|l| l.contains("session/request_permission"));
    let asked = asked.ok_or("no request")?;
    let kept = head.iter().enumerate();
    let kept = kept.filter(|(n, l)| *n < asked || !l.contains("\"outcome\""));
    Ok(kept.map(|(_, l)| *l).collect::<Vec<_>>().join("\n"))
}

/// The key of the waiting card's command well.
fn card(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let keys = shown.iter().filter_map(|e| e.key.as_deref());
    let mut card = keys.filter(|k| k.ends_with(".command") && !k.contains(".step."));
    Ok(card.next().ok_or("no card command")?.to_owned())
}

#[test]
fn the_cards_command_reads_as_shell() -> Check {
    let runtime = window(&waiting_on("t1")?)?;
    let t = tokens(&runtime)?;
    let inks = painted(&runtime, KIND_GLYPH, &card(&runtime)?)?;
    assert!(
        holds(&inks, colour(t.string)),
        "the quoted printf argument is a string"
    );
    Ok(())
}

#[test]
fn a_cell_of_several_lines_is_whole_on_its_card_and_its_closed_row_counts_the_rest() -> Check {
    let runtime = window(&waiting_on("t5")?)?;
    let t = tokens(&runtime)?;
    let card = card(&runtime)?;
    let words = row_text(&runtime.elements(), &card)?;
    assert!(
        words.contains("ratio(1, 0)"),
        "the card holds the last line: {words:?}"
    );
    assert!(holds(
        &painted(&runtime, KIND_GLYPH, &card)?,
        colour(t.keyword)
    ));
    let step = step(&runtime, "t5")?;
    assert_eq!(
        text_of(&runtime, &format!("{step}.target"))?,
        "def ratio(a, b):"
    );
    assert_eq!(text_of(&runtime, &format!("{step}.more"))?, "+2 lines");
    Ok(())
}

#[test]
fn a_tools_json_arguments_read_as_json() -> Check {
    let runtime = window(CHILD)?;
    let call = {
        let hub = runtime.state.hub.try_borrow()?;
        let threads = hub.model.wire.threads.values();
        let mut calls = threads.flat_map(|thread| thread.transcript.tools.values());
        let todo = calls
            .find(|call| call.name == "todo")
            .ok_or("no todo call")?;
        todo.id.as_str().to_owned()
    };
    let mut runtime = runtime;
    let t = tokens(&runtime)?;
    let todo = open(&mut runtime, &call)?;
    let inks = painted(&runtime, KIND_GLYPH, &format!("{todo}.command"))?;
    assert!(
        holds(&inks, colour(t.string)),
        "its strings are JSON strings"
    );
    Ok(())
}

#[test]
fn a_call_taller_than_the_card_scrolls_in_it_and_says_how_much_shows() -> Check {
    // The recorded cell, grown to 41 lines where the recording spells it.
    let grown = "ratio(1, 0)".to_owned() + &"\\nx = 1".repeat(37) + "\\nratio(2, 1)";
    let recording = waiting_on("t5")?.replace("ratio(1, 0)", &grown);
    let runtime = window(&recording)?;
    let card = card(&runtime)?;
    let shown = runtime.elements();
    let well = by_key(&shown, &card).ok_or("no well")?;
    assert!(
        well.rect[3] < 13.0 * 20.0,
        "the well stands 12 lines: {:?}",
        well.rect
    );
    let last = shown.iter().filter_map(|e| e.text.as_deref());
    let last = last
        .filter(|t| t.starts_with("def ratio") && t.ends_with("ratio(2, 1)"))
        .count();
    assert_eq!(last, 1, "every line is there to scroll to");
    let said = shown.iter().filter_map(|e| e.text.as_deref());
    let notice = said
        .filter(|t| t.starts_with("[…] 12 of 41 lines in view"))
        .count();
    assert_eq!(notice, 1, "the cut is named once");
    Ok(())
}
