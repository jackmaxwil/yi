use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col};
use yd_core::feed::{FILES, SourceCmd};
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::FEED;
use super::scenes::sample;

type Sent = Rc<RefCell<Vec<Effect>>>;

/// The changes pane alone in a window over the scene's sample; effects land in the vector.
fn window() -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    let sent = Rc::new(RefCell::new(Vec::new()));
    {
        let mut hub = shared.try_borrow_mut()?;
        sample(&mut hub);
        let into = sent.clone();
        hub.set_sink(move |effect| into.borrow_mut().push(effect));
    }
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        alone,
        (660.0, 900.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    Ok((rt, sent))
}

fn alone(win: &Win, cx: &mut Cx<Win>) -> El {
    match win.hub.try_borrow() {
        Ok(hub) => (super::DEF.view)(
            &PaneCx {
                hub: &hub,
                pane: SCENE_PANE,
                focused: true,
                edits: None,
            },
            cx,
        ),
        Err(_) => col(),
    }
}

/// Keys of the drawn elements, top to bottom.
fn keys(rt: &Runtime<Win>) -> Vec<(String, [f32; 4])> {
    let mut all: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| Some((e.key?, e.rect)))
        .collect();
    all.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]));
    all
}

fn press_text(rt: &mut Runtime<Win>, words: &str) -> Result<(), Box<dyn Error>> {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(words))
        .ok_or(format!("no {words}"))?;
    let [x, y, w, h] = hit.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    Ok(())
}

#[test]
fn a_comment_sits_under_the_line_it_is_about() -> Result<(), Box<dyn Error>> {
    let (rt, _) = window()?;
    let order: Vec<String> = keys(&rt).into_iter().map(|(key, _)| key).collect();
    let at = |end: &str| {
        order.iter().position(|k| {
            k.starts_with("changes-0.row.src/middleware/rate_limit.rs") && k.ends_with(end)
        })
    };
    let comment = order
        .iter()
        .position(|k| k == "changes-0.comment.0")
        .ok_or("no comment")?;
    assert_eq!(at(".L20").map(|l| l < comment), Some(true), "{order:?}");
    assert_eq!(at(".L21").map(|l| l > comment), Some(true), "{order:?}");
    let line = keys(&rt)
        .into_iter()
        .find(|(k, _)| k.ends_with(".L20"))
        .ok_or("no line 20")?;
    assert_eq!(line.1[3], 16.0, "a diff line is 16 tall");
    Ok(())
}

#[test]
fn a_hunk_is_put_back_only_after_the_question_is_answered() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window()?;
    press_text(&mut rt, "Revert")?;
    assert!(
        sent.borrow().is_empty(),
        "a revert went out before it was asked"
    );
    press_text(&mut rt, "Put back")?;
    let sent = sent.borrow();
    let Some(Effect::Source(FEED, SourceCmd::Revert { file, hunk })) = sent.first() else {
        return Err(format!("no revert sent: {sent:?}").into());
    };
    assert_eq!(
        file.to_str(),
        Some("src/routes/upload.rs"),
        "the new file was offered a revert"
    );
    assert!(hunk.lines().any(|l| l.starts_with('+')), "{hunk}");
    Ok(())
}

#[test]
fn pressing_a_changed_files_name_opens_it_in_the_code_viewer() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window()?;
    press_text(&mut rt, "src/routes/upload.rs")?;
    let read = SourceCmd::Read("src/routes/upload.rs".into());
    assert_eq!(*sent.borrow(), [Effect::Source(FILES, read)]);
    Ok(())
}

#[test]
fn a_diff_line_is_found_as_that_line_and_a_header_as_its_file() -> Result<(), Box<dyn Error>> {
    let hub = Hub::new(&[], Vec::new());
    let hub = hub.try_borrow()?;
    let find = |key: &str| super::thing_at(&hub, SCENE_PANE, key).map(|t| t.address.to_string());
    let line = find("changes-3.row.src/middleware/rate_limit.rs:0:5.L18");
    assert_eq!(
        line.as_deref(),
        Some("yi://lines/src/middleware/rate_limit.rs#L18")
    );
    let header = find("changes-3.row.src/middleware/rate_limit.rs");
    assert_eq!(
        header.as_deref(),
        Some("yi://file/src/middleware/rate_limit.rs")
    );
    assert_eq!(find("changes-3.comment.1"), None, "a comment is no row");
    Ok(())
}

/// The sample's changes as a Yi checkout's, with the `git` source's `merge` row: what a merge
/// would take, or `refused` with why.
fn checkout(rt: &mut Runtime<Win>, merge: yd_core::row::Row) -> Result<(), Box<dyn Error>> {
    use yd_core::feed::FeedEvent;
    use yd_core::msg::{Msg, step};
    let repo = yd_core::row::Row::new("repo")
        .text("branch", "yi/0f3e")
        .text("commit", "a1b9f3e")
        .text("base", "main");
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        for row in [repo, merge] {
            step(&mut hub.model, Msg::Feed(FEED, FeedEvent::Upsert(row)));
        }
    }
    rt.invalidate();
    rt.frame(0.0);
    Ok(())
}

fn merge_row() -> yd_core::row::Row {
    yd_core::row::Row::new("merge")
        .text("branch", "yi/0f3e")
        .text("base", "main")
        .text("home", "/w/tally")
        .text("commits", "2")
        .text("files", "3")
        .text("done", "0")
}

/// A press with a frame between each step: a kit button springs, and takes no press otherwise.
fn click(rt: &mut Runtime<Win>, words: &str) -> Result<(), Box<dyn Error>> {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(words));
    let [x, y, w, h] = hit.ok_or(format!("no {words}"))?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.frame(0.016);
    rt.pointer_down(x, y);
    rt.frame(0.016);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    Ok(())
}

fn line_of(rt: &Runtime<Win>) -> String {
    let all = rt.elements();
    let words = all.iter().filter(|e| !e.exiting);
    let words = words.filter(|e| e.path.contains("changes-0.merge-ask"));
    let words: Vec<String> = words.filter_map(|e| e.text.clone()).collect();
    words.join(" ")
}

#[test]
fn merge_into_main_asks_first_naming_what_it_takes_then_merges() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window()?;
    checkout(&mut rt, merge_row())?;
    click(&mut rt, "Merge into main")?;
    assert!(sent.borrow().is_empty(), "nothing merges before the answer");
    assert_eq!(
        line_of(&rt),
        "Merge yi/0f3e into main in /w/tally? 2 commits, 3 files. A local merge: nothing is pushed. Merge Cancel"
    );
    press_text(&mut rt, "Merge")?;
    let merge = yd_core::command::table::Run::new(yd_core::command::tools::changes::MERGE);
    assert_eq!(
        *sent.borrow(),
        [Effect::Source(FEED, SourceCmd::Run(merge))]
    );
    let said = "Merged yi/0f3e into main in /w/tally: 3 files.";
    checkout(&mut rt, merge_row().text("done", "1").text("said", said))?;
    assert_eq!(line_of(&rt), [said, " OK"].concat());
    Ok(())
}

#[test]
fn a_merge_that_cannot_be_made_says_why_and_offers_none() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window()?;
    let why = "The project folder has 2 uncommitted changes: commit or stash them first.";
    checkout(&mut rt, merge_row().text("refused", why))?;
    click(&mut rt, "Merge into main")?;
    assert_eq!(line_of(&rt), [why, " OK"].concat());
    assert!(sent.borrow().is_empty());
    Ok(())
}

/// Dogfood: in a 240 point Changes pane the checkout's long name pushed the button out.
#[test]
fn the_merge_button_stays_inside_a_narrow_pane() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    rt.set_viewport((240.0, 900.0), 1.0);
    let repo = yd_core::row::Row::new("repo")
        .text("branch", "yi/01a1e3bf-507f-7a00-88ba-665daf1488d2")
        .text("commit", "c015abe")
        .text("base", "main");
    {
        use yd_core::msg::{Msg, step};
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let event = yd_core::feed::FeedEvent::Upsert(repo);
        step(&mut hub.model, Msg::Feed(FEED, event));
    }
    checkout(&mut rt, merge_row())?;
    rt.frame(0.016);
    let all = rt.elements();
    let button = all
        .iter()
        .find(|e| e.key.as_deref() == Some("changes-0.merge"));
    let [x, _, w, _] = button.ok_or("no merge button")?.rect;
    assert!(x >= 0.0 && x + w <= 240.0, "the button is cut: {x} + {w}");
    Ok(())
}

fn typing(rt: &mut Runtime<Win>, key: rune::Key) {
    rt.key(rune::KeyEvent {
        key,
        mods: rune::Mods::default(),
    });
    rt.frame(0.016);
}

/// Plan default 17: a hunk is commented as a whole, and the note rides with its lines.
#[test]
fn comment_on_a_hunk_puts_its_lines_and_a_note_in_the_tray() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    press_text(&mut rt, "Comment")?;
    rt.frame(0.016);
    typing(&mut rt, rune::Key::Text("Why 20?".into()));
    typing(&mut rt, rune::Key::Enter);
    let hub = rt.state.hub.try_borrow()?;
    let [thing] = hub.model.attached.things.as_slice() else {
        return Err(format!("attached: {:?}", hub.model.attached.things).into());
    };
    assert_eq!(
        thing.address.to_string(),
        "yi://lines/src/middleware/rate_limit.rs#L1-L23"
    );
    let excerpt = thing.excerpt.as_deref().unwrap_or_default();
    assert!(
        excerpt.contains("Why 20?") && excerpt.contains("+const LIMIT: u32 = 20;"),
        "{excerpt}"
    );
    // Dogfood: the agent said the comment "never reached me, only the code did".
    let sent = yd_core::address::attach::blocks(&hub.model.attached.things);
    let said: String = sent.iter().filter_map(|b| b["text"].as_str()).collect();
    let mine = "My review comment on rate_limit.rs lines 1-23: \"Why 20?\"";
    assert!(said.contains(mine), "{said}");
    Ok(())
}

/// The pane's minimum width: what a person reads or presses is whole and on top of nothing.
const NARROW: f32 = 160.0;

fn rect_of(rt: &Runtime<Win>, words: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(words));
    Ok(hit.ok_or(format!("no {words}"))?.rect)
}

fn apart(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] + a[2] <= b[0] + 0.5
        || b[0] + b[2] <= a[0] + 0.5
        || a[1] + a[3] <= b[1] + 0.5
        || b[1] + b[3] <= a[1] + 0.5
}

/// Every drawn text of a diff line, by its rect.
fn code_texts(rt: &Runtime<Win>) -> Vec<(String, [f32; 4])> {
    let all = rt.elements();
    let lines = all.iter().filter(|e| {
        !e.exiting && e.path.contains("changes-0.row.") && e.path.contains(".L") && e.text.is_some()
    });
    lines
        .filter_map(|e| Some((e.text.clone()?, e.rect)))
        .filter(|(text, _)| !["Revert", "Comment", "Put back", "Keep"].contains(&text.as_str()))
        .collect()
}

#[test]
fn a_narrow_changes_pane_wraps_its_lines_and_cuts_no_control() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    rt.set_viewport((NARROW, 3000.0), 1.0);
    rt.frame(0.016);
    for (text, rect) in code_texts(&rt) {
        assert!(
            rect[0] + rect[2] <= NARROW + 0.5,
            "{text:?} runs out: {rect:?}"
        );
        let whole = rune::text::advance(&text, yd_ui::theme::look::CODE_LINE) <= rect[2] + 0.5;
        assert!(
            whole || rect[3] > 16.5,
            "{text:?} is cut, not wrapped: {rect:?}"
        );
        // Dogfood: beside Comment and Revert a hunk's first line wrapped a letter a line.
        assert!(whole || rect[2] >= 60.0, "{text:?} is squeezed: {rect:?}");
    }
    for words in ["Revert", "Comment"] {
        let control = rect_of(&rt, words)?;
        assert!(control[0] + control[2] <= NARROW + 0.5, "{words} is cut");
        for (text, rect) in code_texts(&rt) {
            assert!(apart(control, rect), "{words} lies over {text:?}");
        }
    }
    press_text(&mut rt, "Revert")?;
    for _ in 0..3 {
        rt.frame(0.016);
    }
    let asked = rect_of(&rt, "Put these lines back as they were?")?;
    for words in ["Put back", "Keep"] {
        let control = rect_of(&rt, words)?;
        assert!(control[0] + control[2] <= NARROW + 0.5, "{words} is cut");
        assert!(apart(control, asked), "{words} lies over the question");
    }
    // The dogfood round's names: a session id for a branch and a deep project folder.
    let long = merge_row()
        .text("branch", "yi/01a1e3bf-507f-7a00-88ba-665daf1488d2")
        .text(
            "home",
            "/Users/you/Development/dogfood/inst/r2a/projects/tally",
        );
    checkout(&mut rt, long)?;
    click(&mut rt, "Merge into main")?;
    let all = rt.elements();
    let asked = all.iter().find(|e| {
        !e.exiting
            && e.text
                .as_deref()
                .is_some_and(|t| t.starts_with("Merge yi/01a1"))
    });
    let asked = asked.ok_or("no merge question")?.rect;
    assert!(
        asked[0] + asked[2] <= NARROW + 0.5,
        "the question runs out: {asked:?}"
    );
    for words in ["Merge", "Cancel"] {
        let control = rect_of(&rt, words)?;
        assert!(control[0] + control[2] <= NARROW + 0.5, "{words} is cut");
        assert!(
            apart(control, asked),
            "{words} lies over the question: {asked:?}"
        );
    }
    Ok(())
}

/// Dogfood: "Merged … OK" was still shown in another thread's Changes after a switch.
#[test]
fn a_merges_result_shows_only_in_its_own_threads_changes() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    let (one, two) = (
        include_str!("../../../../../fixtures/wire/approval-allow.jsonl"),
        include_str!("../../../../../fixtures/wire/approval-reject.jsonl"),
    );
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let _ = yd_core::wire::replay::play(&mut hub.model, one);
        let first = yd_core::ids::ThreadId::new("01a1de97-c166-7d2e-9ca8-5dee496589e1");
        yd_ui::shown::pick(&mut hub, WindowId(0), first);
    }
    checkout(&mut rt, merge_row())?;
    click(&mut rt, "Merge into main")?;
    press_text(&mut rt, "Merge")?;
    let said = "Merged yi/0f3e into main in /w/tally: 3 files.";
    checkout(&mut rt, merge_row().text("done", "1").text("said", said))?;
    assert_eq!(line_of(&rt), [said, " OK"].concat());
    let other = yd_core::ids::ThreadId::new("01a1de97-c223-744c-8afa-018c5c9ad664");
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let _ = yd_core::wire::replay::play(&mut hub.model, two);
        yd_ui::shown::pick(&mut hub, WindowId(0), other);
    }
    rt.invalidate();
    rt.frame(0.016);
    assert_eq!(line_of(&rt), "", "the other thread shows this merge");
    Ok(())
}

/// The words of the merge line, and their box.
fn merge_words(rt: &Runtime<Win>) -> Result<(String, [f32; 4]), Box<dyn Error>> {
    let all = rt.elements();
    let words = all.iter().find(|e| {
        let text = e.text.as_deref().unwrap_or_default();
        !e.exiting && e.path.contains("changes-0.merge-ask") && text.starts_with("Merge")
    });
    let words = words.ok_or("no merge line")?;
    Ok((words.text.clone().unwrap_or_default(), words.rect))
}

/// Dogfood: a project path with no break in it ran out of its box, under "Merge" and "OK".
#[test]
fn a_long_project_path_in_the_merge_line_stays_inside_its_box() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    rt.set_viewport((NARROW, 3000.0), 1.0);
    let home = "~/Development/yi-desktop-wt/dogfood/inst/r3/projects/invoices";
    checkout(&mut rt, merge_row().text("home", home))?;
    click(&mut rt, "Merge into main")?;
    let said = format!("Merged yi/0f3e into main in {home}: 2 files.");
    for step in 0..2 {
        if step == 1 {
            press_text(&mut rt, "Merge")?;
            checkout(&mut rt, merge_row().text("done", "1").text("said", &said))?;
        }
        let (words, [_, _, w, h]) = merge_words(&rt)?;
        let options = rune::text::RichLayoutOptions::new().wrap_anywhere();
        let need =
            rune::text::measure_with(&words, &[], yd_ui::theme::look::META, Some(w), options);
        assert!(
            h + 0.5 >= need.1,
            "{words:?} needs {need:?} and has {w} by {h}"
        );
    }
    Ok(())
}

/// Dogfood: "Please also add a test for 0 percent dis" was cut at the narrow pane's edge.
#[test]
fn a_comment_and_its_draft_wrap_inside_a_narrow_pane() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = window()?;
    rt.set_viewport((NARROW, 3000.0), 1.0);
    let said = "Please also add a test for 0 percent discount";
    press_text(&mut rt, "Comment")?;
    rt.frame(0.016);
    typing(&mut rt, rune::Key::Text(said.into()));
    let mut seen = 0;
    for kept in [false, true] {
        if kept {
            typing(&mut rt, rune::Key::Enter);
        }
        let all = rt.elements();
        let part = if kept { ".comment." } else { ".draft" };
        let words = all.iter().filter(|e| !e.exiting && e.path.contains(part));
        for e in words.filter(|e| e.text.as_deref() == Some(said)) {
            let [x, _, w, h] = e.rect;
            let need = rune::text::measure(said, &[], yd_ui::theme::look::LABEL, Some(w));
            assert!(x + w <= NARROW + 0.5, "{part} runs out: {:?}", e.rect);
            assert!(
                h + 0.5 >= need.1,
                "{part} is cut: needs {need:?}, has {w} by {h}"
            );
            seen += 1;
        }
    }
    assert_eq!(seen, 2, "the draft and the kept comment");
    let all = rt.elements();
    let kept = all
        .iter()
        .filter(|e| !e.exiting && e.path.contains(".comment."));
    for e in kept.filter(|e| e.text.is_some()) {
        assert!(
            e.rect[0] + e.rect[2] <= NARROW + 0.5,
            "{:?} runs out",
            e.text
        );
    }
    Ok(())
}

/// Dogfood: a person does not know to ask for a commit before Merge into main can work.
#[test]
fn uncommitted_work_offers_to_ask_the_agent_to_commit() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = window()?;
    {
        let mut hub = rt.state.hub.try_borrow_mut()?;
        let one = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
        let _ = yd_core::wire::replay::play(&mut hub.model, one);
        let first = yd_core::ids::ThreadId::new("01a1de97-c166-7d2e-9ca8-5dee496589e1");
        yd_ui::shown::pick(&mut hub, WindowId(0), first);
    }
    let why = "The agent's changes are not committed yet. Ask it to commit, then merge.";
    let row = merge_row().text("refused", why).text("uncommitted", "1");
    checkout(&mut rt, row)?;
    click(&mut rt, "Merge into main")?;
    assert_eq!(line_of(&rt), [why, " Ask it to commit OK"].concat());
    sent.borrow_mut().clear();
    click(&mut rt, "Ask it to commit")?;
    let sent = sent.borrow();
    let asked = sent.iter().any(|effect| {
        matches!(effect, Effect::Wire(request)
            if request.line.contains("session/prompt") && request.line.contains("commit"))
    });
    assert!(asked, "no prompt to commit: {sent:?}");
    assert_eq!(line_of(&rt), "", "the line goes once asked");
    Ok(())
}
