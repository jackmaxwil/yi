//! Big inputs open: a 5 MB file in the code viewer and a 20,000-line diff in Changes, each
//! read by its real source, with the time each first frame took in `numbers`.

mod support;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use rune::{ElementInfo, Runtime};
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{CHANGES, FILES, FeedEvent};
use yd_core::msg::Msg;
use yd_core::row::Value;
use yd_ui::win::{Win, dispatch};

use support::scene::{WINDOW, runtime};

type Check = Result<(), Box<dyn Error>>;

/// A folder of this test's own under the lane's `target/`, made afresh.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/volume")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn lines(count: usize) -> String {
    (0..count)
        .map(|n| format!("let value_{n:06} = {n};\n"))
        .collect()
}

fn found<'a>(all: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    all.iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn timed(rt: &mut Runtime<Win>) -> f64 {
    let start = Instant::now();
    rt.frame(0.016);
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
fn a_five_megabyte_file_opens_and_says_where_it_was_cut() -> Check {
    let dir = folder("big-file")?;
    let text = lines(220_000);
    assert!(text.len() > 5_000_000, "{} bytes", text.len());
    std::fs::write(dir.join("big.rs"), &text)?;
    let read = yd_sources::fs::read(&dir, None, "big.rs");
    assert_eq!(read.get("truncated"), Some(&Value::Flag(true)));
    let mut rt = runtime("code/empty", WINDOW)?;
    let file = Target::Thing("yi://file/big.rs".parse()?);
    rt.update(move |win: &mut Win, _| {
        dispatch(
            win,
            Msg::Command(Route::Model, Run::new(OPEN_FILE).on(file)),
        );
        dispatch(win, Msg::Feed(FILES, FeedEvent::Upsert(read)));
    });
    let first = timed(&mut rt);
    let then = (0..10).map(|_| timed(&mut rt)).fold(0.0, f64::max);
    println!("5 MB file: first frame {first:.1} ms, slowest of the next ten {then:.1} ms");
    let all = rt.elements();
    assert!(
        found(&all, "code-0.L1").is_some(),
        "the file's first line is not drawn"
    );
    let mut words = all.iter().filter_map(|e| e.text.as_deref());
    let cut = words.find(|t| t.starts_with("[…] first"));
    let cut = cut.ok_or("the cut is not said")?;
    assert!(
        cut.contains("2 MiB") && cut.contains("open it in an editor"),
        "{cut}"
    );
    Ok(())
}

#[allow(
    clippy::disallowed_methods,
    reason = "the test makes a real repository for the git source to read"
)]
fn git(dir: &Path, args: &[&str]) -> Check {
    let done = Command::new("git").current_dir(dir).args(args).output()?;
    assert!(done.status.success(), "git {args:?}: {done:?}");
    Ok(())
}

#[test]
fn a_twenty_thousand_line_diff_opens() -> Check {
    let dir = folder("big-diff")?;
    git(&dir, &["init", "-q", "-b", "main"])?;
    std::fs::write(dir.join("big.rs"), "")?;
    git(&dir, &["add", "big.rs"])?;
    let who = [
        "-c",
        "user.name=volume",
        "-c",
        "user.email=volume@example.invalid",
    ];
    git(
        &dir,
        &[who[0], who[1], who[2], who[3], "commit", "-qm", "Seed"],
    )?;
    std::fs::write(dir.join("big.rs"), lines(20_000))?;
    let rows = yd_sources::git::snapshot(&dir)?;
    let mut rt = runtime("changes/empty", WINDOW)?;
    rt.update(move |win: &mut Win, _| dispatch(win, Msg::Feed(CHANGES, FeedEvent::Reset(rows))));
    let first = timed(&mut rt);
    let then = (0..10).map(|_| timed(&mut rt)).fold(0.0, f64::max);
    println!("20,000-line diff: first frame {first:.1} ms, slowest of the next ten {then:.1} ms");
    let all = rt.elements();
    let words: Vec<&str> = all.iter().filter_map(|e| e.text.as_deref()).collect();
    assert!(
        words.iter().any(|t| t.contains("let value_000000")),
        "the diff's first line is not drawn"
    );
    Ok(())
}
