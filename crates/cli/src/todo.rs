use std::path::Path;

use yi_runtime::session_store::{JsonlRepo, SessionRepo};
use yi_runtime::todo::{latest_record, text};
use yi_types::todo::TodoRecord;

fn newest_record(session_dir: &Path, cwd: &Path) -> Option<TodoRecord> {
    let mut repo = JsonlRepo::new(
        session_dir.to_path_buf(),
        cwd.to_string_lossy().into_owned(),
    );
    let mut listed = SessionRepo::list(&mut repo).ok()?;
    listed.sort_by_key(|metadata| std::cmp::Reverse(metadata.created_at));
    listed.iter().find_map(|metadata| {
        let session = SessionRepo::open(&mut repo, &metadata.id).ok()?;
        latest_record(&session)
    })
}

pub fn run(args: &crate::Args) -> i32 {
    let cwd = crate::effective_cwd(args);
    match args.prompt.trim() {
        "" | "list" => {
            let Some(record) = newest_record(&crate::default_session_dir(args), &cwd) else {
                println!("no todo list in any session under {}", cwd.display());
                return 0;
            };
            if args.json {
                return match serde_json::to_string_pretty(&record.list) {
                    Ok(text) => {
                        println!("{text}");
                        0
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        1
                    }
                };
            }
            println!("{}", text::render(&record.list));
            println!(
                "touched: {} · last op: {} by {}",
                record.touched, record.op, record.actor
            );
            0
        }
        other => {
            eprintln!("usage: yi todo [list]  (unknown subcommand {other:?})");
            2
        }
    }
}
