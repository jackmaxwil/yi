use std::error::Error;
use std::ops::Range;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use tokio::sync::mpsc::UnboundedSender;
use yi_tui::Command;
use yi_tui::app::{App, TuiOptions};
use yi_tui::assist::{Ask, Assist, AssistSettings, Engine, Words};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::composer::Composer;
use yi_tui::keymap::default_keymap;

use crate::common;
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// A dictionary with one rule: three equal letters in a row are a typo, and the word with that
/// run cut to two is its correction and only guess. It completes from a fixed word list.
struct Fake(&'static [&'static str]);

fn collapse(word: &str) -> Option<String> {
    let chars: Vec<char> = word.chars().collect();
    let tripled = chars.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]);
    let kept =
        (0..chars.len()).filter(|&i| i < 2 || chars[i - 1] != chars[i] || chars[i - 2] != chars[i]);
    tripled.then(|| kept.map(|i| chars[i]).collect())
}

impl Engine for Fake {
    fn typos(&mut self, line: &str) -> Option<Vec<Range<usize>>> {
        let mut spans = Vec::new();
        let mut at = 0;
        for word in line.split(|ch: char| !ch.is_alphabetic()) {
            let len = word.chars().count();
            if collapse(word).is_some() {
                spans.push(at..at + len);
            }
            at += len + 1;
        }
        Some(spans)
    }

    fn words(&mut self, ask: Ask, text: &str) -> Option<Vec<String>> {
        Some(match ask {
            Ask::Complete => (self.0.iter())
                .filter(|w| w.starts_with(text))
                .map(|w| (*w).to_owned())
                .collect(),
            Ask::Guesses | Ask::Correct => collapse(text).into_iter().collect(),
            Ask::Check => Vec::new(),
        })
    }
}

fn assist(words: &'static [&'static str], autocorrect: bool) -> Assist {
    let settings = AssistSettings {
        autocorrect,
        ..AssistSettings::DEFAULT
    };
    Assist::new(settings, Some(Box::new(Fake(words))), Arc::default())
}

fn app_in(cwd: &str, assist: Assist) -> App {
    let options = TuiOptions {
        session_name: "assist".to_owned(),
        cwd: cwd.to_owned(),
        ..common::options()
    };
    let mut app = App::new(
        options,
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    );
    app.set_assist(assist);
    app
}

fn press(app: &mut App, tx: &UnboundedSender<Command>, code: KeyCode, modifiers: KeyModifiers) {
    yi_tui::input::handle_terminal_event(app, tx, Event::Key(KeyEvent::new(code, modifiers)));
}

fn key(app: &mut App, tx: &UnboundedSender<Command>, code: KeyCode) {
    press(app, tx, code, KeyModifiers::NONE);
}

fn type_text(app: &mut App, tx: &UnboundedSender<Command>, text: &str) {
    for ch in text.chars() {
        key(app, tx, KeyCode::Char(ch));
    }
}

fn paste(app: &mut App, tx: &UnboundedSender<Command>, text: &str) {
    yi_tui::input::handle_terminal_event(app, tx, Event::Paste(text.to_owned()));
}

/// The marked runs of `text` with the cursor at its end, painted the way the box paints them.
fn marked(text: &str) -> Vec<String> {
    let mut composer = Composer::default();
    composer.set_assist(assist(&[], false));
    composer.set_text(text);
    composer.sync_assist("/tmp", "");
    composer.set_typo_style(yi_tui::assist::spell_style(ratatui::style::Color::Red));
    let height = composer.layout(60, 40);
    composer.set_frame(Style::default(), Style::default(), Style::default(), None);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 60, height));
    composer.render(Rect::new(0, 0, 60, height), &mut buffer);
    let under = |x: u16, y: u16| {
        buffer
            .cell((x, y))
            .is_some_and(|c| c.modifier.contains(Modifier::UNDERLINED))
    };
    let row = |y: u16| -> String {
        (0..60)
            .filter(|x| under(*x, y))
            .filter_map(|x| buffer.cell((x, y)).map(|c| c.symbol().to_owned()))
            .collect()
    };
    (0..height).map(row).filter(|run| !run.is_empty()).collect()
}

/// Dies with code words underlined: a word inside a fenced block, a path, a snake name or an
/// inline span was checked like prose, and each line of a fence was masked on its own.
#[test]
fn typo_marks_fall_on_prose_and_never_on_code() {
    assert_eq!(marked("see helllo now"), ["helllo"]);
    assert!(
        marked("helllo").is_empty(),
        "the word still being typed is left alone"
    );
    for code in [
        "`helllo` ",
        "foo_barrr ",
        "src/helllo.rs ",
        "/helllo ",
        "hellloWorld ",
    ] {
        assert!(marked(code).is_empty(), "{code}");
    }
    let fenced = "the sshh moment:\n```rust\nlet helllo = 1;\nfn mmmain() {}\n```\nand thhhen ";
    assert_eq!(marked(fenced), ["thhhen"]);
}

/// Dies with Tab inserting a ghost nobody saw: with the cursor two letters before a word's end
/// the old ghost was that word's tail, and Tab turned `hello` into `hello lo`.
#[test]
fn tab_takes_only_a_drawn_ghost_and_one_undo_removes_it() -> TestResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = app_in("/tmp", assist(&["zzqhello"], false));
    type_text(&mut app, &tx, "zzqh");
    assert!(app.ghost_showing());
    key(&mut app, &tx, KeyCode::Tab);
    assert_eq!(app.composer_text(), "zzqhello ");
    press(&mut app, &tx, KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(app.composer_text(), "zzqh");

    let mut mid = app_in("/tmp", assist(&["zzqhello"], false));
    type_text(&mut mid, &tx, "zzqhel");
    key(&mut mid, &tx, KeyCode::Left);
    key(&mut mid, &tx, KeyCode::Left);
    assert!(!mid.ghost_showing(), "no ghost is drawn mid-word");
    key(&mut mid, &tx, KeyCode::Tab);
    assert!(
        !mid.composer_text().contains("lo"),
        "{:?}",
        mid.composer_text()
    );
    Ok(())
}

/// Dies with autocorrect rewriting a word nobody asked it to, or doing nothing once asked.
#[test]
fn autocorrect_rewrites_on_space_only_when_on() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut off = app_in("/tmp", assist(&[], false));
    type_text(&mut off, &tx, "helllo ");
    assert_eq!(off.composer_text(), "helllo ");
    let mut on = app_in("/tmp", assist(&[], true));
    type_text(&mut on, &tx, "say helllo ");
    assert_eq!(on.composer_text(), "say hello ");
}

/// Dies with ctrl+. showing nothing, or Enter on a guess replacing the wrong span.
#[test]
fn ctrl_dot_lists_guesses_and_enter_takes_one() -> TestResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = app_in("/tmp", assist(&[], false));
    type_text(&mut app, &tx, "a helllo");
    press(&mut app, &tx, KeyCode::Char('.'), KeyModifiers::CONTROL);
    let rows = common::live_rows(&mut app)?;
    assert!(rows.iter().any(|row| row.trim() == "> hello"), "{rows:#?}");
    key(&mut app, &tx, KeyCode::Enter);
    assert_eq!(app.composer_text(), "a hello");
    Ok(())
}

/// Dies with a coding agent's prompt rewritten under the user: `::check::` and `10:100:` became
/// emoji. Only a whole `:name:` token typed closed in prose turns into its emoji.
#[test]
fn a_typed_shortcode_becomes_emoji_and_code_keeps_its_colons() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    for (typed, kept) in [
        ("ship it :rocket:", "ship it 🚀"),
        ("nice :ok_hand:", "nice 👌"),
        (
            "call yi_tui::check::run and std::x::y",
            "call yi_tui::check::run and std::x::y",
        ),
        ("at 10:100: ok", "at 10:100: ok"),
        ("`a :joy:`", "`a :joy:`"),
        ("http://h/:joy:", "http://h/:joy:"),
    ] {
        let mut app = app_in("/tmp", assist(&[], false));
        type_text(&mut app, &tx, typed);
        assert_eq!(app.composer_text(), kept, "{typed}");
    }
    let mut pasted = app_in("/tmp", assist(&[], false));
    paste(&mut pasted, &tx, "a :joy: b");
    assert_eq!(
        pasted.composer_text(),
        "a :joy: b",
        "a paste is taken as written"
    );
}

/// Dies with a completion splicing the wrong place: the first whitespace word equal to the token
/// was replaced, with single spaces assumed, so `fix  @ma` lost a space and a repeated token
/// changed its first copy.
#[test]
fn a_file_row_replaces_the_token_at_the_cursor() -> TestResult {
    let dir = Scratch::new("yi-assist-files")?;
    std::fs::write(dir.join("main.rs"), "")?;
    let at = dir.display().to_string();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    for (typed, want) in [
        ("fix  @ma", "fix  main.rs "),
        ("  @ma", "  main.rs "),
        ("@ma and @ma", "@ma and main.rs "),
    ] {
        let mut app = app_in(&at, assist(&[], false));
        type_text(&mut app, &tx, typed);
        key(&mut app, &tx, KeyCode::Tab);
        assert_eq!(app.composer_text(), want, "{typed}");
    }
    Ok(())
}

/// Dies with Up stuck on a recalled `/compact` or `@src` entry: recalling it opened a list, and
/// the next arrow moved the list instead of history.
#[test]
fn arrows_walk_history_through_recalled_slash_and_file_entries() -> TestResult {
    let dir = Scratch::new("yi-assist-history")?;
    std::fs::write(dir.join("src.rs"), "")?;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut keymap = default_keymap();
    keymap.apply_overrides([("up", "history-prev"), ("down", "history-next")])?;
    let options = TuiOptions {
        cwd: dir.display().to_string(),
        ..common::options()
    };
    let mut app = App::new(options, Theme::new(ColorTier::TrueColor, true), keymap, 80);
    app.set_assist(assist(&[], false));
    for entry in ["/compact", "ask @src"] {
        paste(&mut app, &tx, entry);
        key(&mut app, &tx, KeyCode::Enter);
    }
    for want in ["ask @src", "/compact"] {
        key(&mut app, &tx, KeyCode::Up);
        assert_eq!(app.composer_text(), want);
        assert!(!app.bottom_open(), "{want}: a recalled entry opened a list");
    }
    key(&mut app, &tx, KeyCode::Down);
    assert_eq!(app.composer_text(), "ask @src");
    Ok(())
}

/// Dies with a skill named like a verb taking the verb over: `/quit` became a prompt to read
/// that skill. Skills list under `$`, and every `/` row is a built-in.
#[test]
fn a_skill_named_like_a_verb_lists_under_dollar_not_slash() -> TestResult {
    let dir = Scratch::new("yi-assist-skills")?;
    let skill = dir.join(".yi/skills/quit");
    std::fs::create_dir_all(&skill)?;
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: quit\ndescription: a skill that quits\n---\nbody\n",
    )?;
    let at = dir.display().to_string();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut slash = app_in(&at, assist(&[], false));
    type_text(&mut slash, &tx, "/qui");
    let rows = common::live_rows(&mut slash)?;
    assert!(
        rows.iter().any(|row| row.contains("leave the session")),
        "{rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("a skill that quits")),
        "{rows:#?}"
    );
    let mut dollar = app_in(&at, assist(&[], false));
    type_text(&mut dollar, &tx, "$qu");
    key(&mut dollar, &tx, KeyCode::Tab);
    assert_eq!(dollar.composer_text(), "$quit ");
    Ok(())
}

/// Dies with JSON keys offered as words (`timestamp`, `customType`) or with no words at all from
/// the per-folder store. The scrubber masked every entry id, so the copy gets them back unique.
#[test]
fn the_word_list_reads_user_text_from_the_per_folder_store() -> TestResult {
    let dir = Scratch::new("yi-assist-words")?;
    let folder = dir.join("--repo--");
    std::fs::create_dir_all(&folder)?;
    let recorded = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sessions/01a0648f.jsonl");
    let scrubbed = std::fs::read_to_string(recorded)?;
    let lines = scrubbed
        .lines()
        .enumerate()
        .map(|(n, line)| line.replacen("\"id\":\"[MASKED]\"", &format!("\"id\":\"e{n}\""), 1));
    let restored: String = lines.map(|line| line + "\n").collect();
    std::fs::write(folder.join("2026-01-01T00-00-00_01a0648f.jsonl"), restored)?;
    let words = Words::from_sessions(&dir);
    assert_eq!(words.suffix("avail").as_deref(), Some("able"));
    assert_eq!(words.suffix("ipyt").as_deref(), Some("hon"));
    for key in ["timest", "customt", "creat"] {
        assert_eq!(words.suffix(key), None, "{key}: a JSON key was learned");
    }
    assert_eq!(
        Words::from_sessions(&dir.join("missing")).suffix("av"),
        None
    );
    Ok(())
}

/// Dies with the mark drawn as a plain red underline: the cell crossterm actually writes for the
/// spelling style must carry the curl, and an ordinary underline must stay straight.
#[test]
fn the_spelling_mark_reaches_the_terminal_as_a_curl() -> TestResult {
    use ratatui::backend::{Backend, CrosstermBackend};
    let draw = |style: Style| -> Result<Vec<u8>, Box<dyn Error>> {
        let mut out = Vec::new();
        {
            let mut backend = CrosstermBackend::new(yi_tui::term::Squiggle::new(&mut out));
            let mut cell = ratatui::buffer::Cell::default();
            cell.set_symbol("x").set_style(style);
            backend.draw(std::iter::once((0, 0, &cell)))?;
            Backend::flush(&mut backend)?;
        }
        Ok(out)
    };
    let curls = |bytes: &[u8]| bytes.windows(6).filter(|w| *w == b"\x1b[4:3m").count();
    let spelled = draw(yi_tui::assist::spell_style(ratatui::style::Color::Rgb(
        255, 95, 95,
    )))?;
    assert_eq!(
        curls(&spelled),
        1,
        "{:?}",
        String::from_utf8_lossy(&spelled)
    );
    let link = draw(Style::default().add_modifier(Modifier::UNDERLINED))?;
    assert_eq!(curls(&link), 0, "{:?}", String::from_utf8_lossy(&link));
    Ok(())
}
