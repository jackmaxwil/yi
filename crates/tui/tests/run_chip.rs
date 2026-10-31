use std::error::Error;

use ratatui::text::Line;
use yi_tui::cell::{Cell, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::markdown::{render, render_answer, run_chip, shell_blocks};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// An answer shaped like the one that asked for this: a fix the reader runs themselves, one long
/// chained command, then fences that are not for a shell and a paragraph spelling the chip.
const REPLY: &str = "Fix (on your side): run this from the main checkout:\n\n\
```bash\ncd ~/Development/yi && git pull --ff-only origin main && scripts/build_dist.sh && \\\n  \
cp target/dist/yi ~/.local/bin/yi && yi --version\n```\n\n\
```rust\nfn main() {}\n```\n\n\
```\nunlabelled\n```\n\n\
```sh title=check\necho hi\n```\n\n\
The button reads ▶ run, and a fence with nothing in it is still a fence:\n\n\
```zsh\n```\n";

/// Dies with a chip on every surface that renders markdown, where a viewer or a solo transcript
/// would draw a button nothing can press, or with a chip on a fence that is not a shell's.
#[test]
fn only_a_surface_that_can_press_it_gets_a_chip_and_only_on_shell_fences() -> TestResult {
    let theme = theme();
    let quiet = render(REPLY, 60, &theme);
    assert!(
        quiet.iter().all(|line| run_chip(line).is_none()),
        "plain rendering drew a chip"
    );
    assert!(
        render_answer(REPLY, 60, &theme)
            .iter()
            .all(|line| run_chip(line).is_none()),
        "an answer drew a chip on a surface that cannot press it"
    );

    let mut pressable = theme;
    pressable.run = true;
    let rows = render_answer(REPLY, 60, &pressable);
    let chipped: Vec<String> = rows
        .iter()
        .filter(|line| run_chip(line).is_some())
        .map(|line| text(line).trim_start_matches("│ ").to_owned())
        .collect();
    assert_eq!(
        chipped,
        ["bash  ▶ run ", "sh title=check  ▶ run ", "zsh  ▶ run "],
        "one chip per shell fence, none on rust, unlabelled or the paragraph"
    );
    assert!(
        rows.iter()
            .any(|line| text(line).contains("The button reads ▶ run")),
        "the paragraph that spells the chip is still drawn"
    );
    Ok(())
}

/// Dies with a chip that points at another fence's command: the console recovers the code by the
/// chip's ordinal, so the ordinals must follow the order the fences are drawn in.
#[test]
fn the_shell_blocks_are_the_fences_in_the_order_their_chips_are_drawn() {
    assert_eq!(
        shell_blocks(REPLY),
        [
            "cd ~/Development/yi && git pull --ff-only origin main && scripts/build_dist.sh && \\\n  \
             cp target/dist/yi ~/.local/bin/yi && yi --version\n",
            "echo hi\n",
            "",
        ]
    );
}

/// Dies with a chip drawn where its columns are not: the press resolves against the span's
/// start, which the answer's own gutter pushes right.
#[test]
fn the_chip_reports_the_columns_it_is_drawn_in() -> TestResult {
    let mut themed = theme();
    themed.run = true;
    let lines = Cell::Assistant {
        markdown: REPLY.to_owned(),
    }
    .lines(60, &themed, TranscriptMode::Normal, 0);
    let row = lines
        .iter()
        .find(|line| run_chip(line).is_some())
        .ok_or("no chip on the assistant cell")?;
    let (from, to) = run_chip(row).ok_or("no chip")?;
    let drawn: String = text(row).chars().skip(from).take(to - from).collect();
    assert_eq!(drawn, " ▶ run ");

    let plain = Cell::Assistant {
        markdown: REPLY.to_owned(),
    }
    .lines(60, &theme(), TranscriptMode::Normal, 0);
    assert!(plain.iter().all(|line| run_chip(line).is_none()));
    Ok(())
}

const TWO: &str = "First, update:\n\n```bash\ngit pull first\n```\n\nThen build:\n\n\
```sh\nscripts/build second\n```\n\nThat is all.\n";

/// Dies with a chip that runs its neighbour's command once a scroll clips the cell's first chip
/// off the top: the ordinal counts the chips above the window too, or the second row would
/// answer as the first.
#[test]
fn each_painted_chip_runs_the_fence_it_sits_on_at_every_scroll() -> TestResult {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let mut themed = theme();
    themed.run = true;
    let mut app = yi_tui::app::App::new(
        crate::common::options(),
        themed,
        yi_tui::keymap::default_keymap(),
        80,
    );
    app.commit_cell(&Cell::User {
        text: "update".to_owned(),
    });
    app.commit_cell(&Cell::Assistant {
        markdown: TWO.to_owned(),
    });
    for i in 0..12 {
        app.commit_cell(&Cell::Assistant {
            markdown: format!("filler {i}\n"),
        });
    }
    let area = Rect::new(2, 0, 50, 12);
    let mut seen = std::collections::BTreeSet::new();
    for scroll in 0..40 {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 12));
        let mut held = scroll;
        yi_tui::render::paint_pane(&mut app, None, &mut buffer, area, &mut held);
        for (rect, code) in app.run_chips() {
            let row: String = (0..60)
                .filter_map(|x| {
                    buffer
                        .cell((x, rect.y))
                        .map(|cell| cell.symbol().to_owned())
                })
                .collect();
            let label = if code.contains("first") { "bash" } else { "sh" };
            assert!(
                row.contains(label) && row.contains("▶ run"),
                "scroll {scroll}: {code:?} runs from {row:?}"
            );
            let chip: String = (rect.x..rect.right())
                .filter_map(|x| {
                    buffer
                        .cell((x, rect.y))
                        .map(|cell| cell.symbol().to_owned())
                })
                .collect();
            assert_eq!(chip, " ▶ run ", "scroll {scroll}: the rect misses the chip");
            seen.insert(code);
        }
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        ["git pull first\n", "scripts/build second\n"],
        "both chips were reachable by scrolling"
    );
    Ok(())
}
