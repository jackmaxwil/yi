//! `yi fetch <url>`: one resolve through the wall, printed.

use std::sync::Arc;

use crate::{Args, McpOneShot, configured_plans_dir, effective_cwd, home};

pub(crate) fn run(args: &Args) -> i32 {
    let target = args.prompt.trim();
    if target.is_empty() {
        eprintln!("usage: yi fetch <url>");
        return 2;
    }
    let url: yi_types::url::Url = match target.parse() {
        Ok(url) => url,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    let workspace = effective_cwd(args);
    let mut resolver =
        yi_runtime::fetch::Resolver::new(workspace.clone(), yi_runtime::Wall::default())
            .with_home(home().to_path_buf());
    if let Some(dir) = configured_plans_dir(&workspace) {
        resolver = resolver.with_plans_dir(dir);
    }
    resolver = resolver.with_mcp_read(Arc::new(McpOneShot));
    match resolver.fetch(&url) {
        Ok(fetched) => {
            print!("{}", fetched.text);
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}
