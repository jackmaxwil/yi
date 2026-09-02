use std::path::PathBuf;

use yi_runtime::plan::why::{Answer, answer, commits_for};

pub struct Options {
    pub cwd: PathBuf,
    pub plans_dir: Option<PathBuf>,
    pub json: bool,
}

pub fn run(target: &str, options: &Options) -> i32 {
    let target = target.trim();
    if target.is_empty() {
        eprintln!("usage: yi why <file>:<line> | yi why <plan>/<todo>");
        return 2;
    }
    match split_location(target) {
        Some((path, line)) => forward(path, line, options),
        None => reverse(target, options),
    }
}

fn split_location(target: &str) -> Option<(&str, u32)> {
    let (path, line) = target.rsplit_once(':')?;
    line.parse::<u32>().ok().map(|line| (path, line))
}

fn plans_dir(options: &Options) -> PathBuf {
    options
        .plans_dir
        .clone()
        .unwrap_or_else(|| options.cwd.join(yi_runtime::plan::PLANS_DIR))
}

fn forward(path: &str, line: u32, options: &Options) -> i32 {
    let resolved = match answer(&options.cwd, &plans_dir(options), path, line) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if options.json {
        println!(
            "{}",
            serde_json::json!({
                "commit": resolved.commit,
                "subject": resolved.subject,
                "plan": resolved.plan.as_ref().map(yi_types::plan::doc::PlanId::as_str),
                "todo": resolved.todo,
                "goal": resolved.goal,
            })
        );
        return 0;
    }
    print(&resolved);
    0
}

fn print(resolved: &Answer) {
    let short = resolved.commit.get(..12).unwrap_or(&resolved.commit);
    println!("{short}  {}", resolved.subject);
    match (&resolved.plan, &resolved.todo) {
        (Some(plan), Some(todo)) => {
            println!("  todo  {todo}");
            println!("  plan  {plan}");
            match &resolved.goal {
                Some(goal) => println!("  goal  {goal}"),
                None => println!("  goal  (the plan file the trailer names is not on disk)"),
            }
        }
        _ => println!("  the commit carries no Plan: trailer, so this line answers to no todo"),
    }
}

fn reverse(address: &str, options: &Options) -> i32 {
    let found = match commits_for(&options.cwd, address) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if options.json {
        let rows: Vec<serde_json::Value> = found
            .iter()
            .map(|(commit, subject)| serde_json::json!({ "commit": commit, "subject": subject }))
            .collect();
        println!(
            "{}",
            serde_json::json!({ "todo": address, "commits": rows })
        );
        return 0;
    }
    if found.is_empty() {
        println!("no commit carries plan://{address}");
        return 0;
    }
    for (commit, subject) in &found {
        println!("{}  {subject}", commit.get(..12).unwrap_or(commit));
    }
    0
}
