use std::io::Write;
use std::path::Path;

use crate::Args;

pub(crate) fn run(args: &Args) -> i32 {
    if args.prompt.trim() == "download" {
        return download(crate::user_config::home());
    }
    #[cfg(feature = "tui")]
    {
        yi_tui::assist::report(crate::user_config::home(), args.prompt.trim())
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = args.prompt;
        eprintln!("word engines need the tui build");
        2
    }
}

fn download(home: &Path) -> i32 {
    let dest = home.join(yi_types::config::SMOL_REL);
    if let Some(parent) = dest.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: {error}");
        return 1;
    }
    let response = match ureq::get(yi_types::config::SMOL_URL).call() {
        Ok(response) => response,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let mut file = match std::fs::File::create(&dest) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let mut reader = response.into_reader();
    if let Err(error) = std::io::copy(&mut reader, &mut file) {
        eprintln!("error: {error}");
        return 1;
    }
    if let Err(error) = file.flush() {
        eprintln!("error: {error}");
        return 1;
    }
    println!("saved {}", dest.display());
    0
}
