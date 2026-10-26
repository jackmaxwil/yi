use std::path::Path;

use crate::environment::PROBE;

pub fn tracked(root: &Path, paths: &[String]) -> Vec<bool> {
    let Ok(root) = std::fs::canonicalize(root) else {
        return vec![false; paths.len()];
    };
    let rels: Vec<Option<String>> = paths
        .iter()
        .map(|path| {
            let canonical = std::fs::canonicalize(path).ok()?;
            let rel = canonical.strip_prefix(&root).ok()?;
            Some(rel.to_string_lossy().into_owned())
        })
        .collect();
    let mut args = vec!["ls-files", "-z", "--"];
    args.extend(rels.iter().flatten().map(String::as_str));
    if args.len() == 3 {
        return vec![false; paths.len()];
    }
    let listed: std::collections::HashSet<String> =
        crate::lane::capture(&root, "git", &args, PROBE)
            .map(|out| {
                out.split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
    rels.iter()
        .map(|rel| rel.as_deref().is_some_and(|rel| listed.contains(rel)))
        .collect()
}

pub fn branch_diff(root: &Path) -> Option<yi_types::lane::BranchDiff> {
    branch_diff_capped(root, 4 << 20)
}

/// The committed branch only: `/land` pushes commits, so an uncommitted edit is not in it.
/// `cap` bounds the patch in bytes, and a cut ends it on a whole file with a `[…]` row.
pub fn branch_diff_capped(root: &Path, cap: usize) -> Option<yi_types::lane::BranchDiff> {
    let git = |args: &[&str]| crate::lane::capture(root, "git", args, PROBE).ok();
    let whole = |args: &[&str]| {
        let label = format!("git {}", args.join(" "));
        crate::lane::capture_named(
            root,
            &label,
            "git",
            args,
            PROBE,
            cap.saturating_mul(2),
            None,
        )
        .ok()
    };
    let base = git(&["merge-base", "HEAD", "origin/main"])
        .or_else(|| git(&["merge-base", "HEAD", "main"]))?
        .trim()
        .to_owned();
    let numstat = whole(&["diff", "--numstat", &base, "HEAD"])?;
    let listed = if numstat.truncated {
        let head = prefix(&numstat.stdout, cap);
        head.rfind('\n')
            .and_then(|end| head.get(..end))
            .unwrap_or("")
    } else {
        numstat.stdout.as_str()
    };
    let files: Vec<(String, u64, u64)> = listed
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            Some((parts.next()?.to_owned(), added, removed))
        })
        .collect();
    let mut patch = whole(&["diff", "--no-color", &base, "HEAD"])?.stdout;
    if patch.len() > cap {
        // Invariant: a patch line is prefixed, so only a file header starts a line with `diff --git `.
        let head = prefix(&patch, cap);
        let head = head
            .rfind("\ndiff --git ")
            .and_then(|end| head.get(..=end))
            .unwrap_or("");
        let shown = head
            .lines()
            .filter(|line| line.starts_with("diff --git "))
            .count();
        let over = if numstat.truncated { "over " } else { "" };
        patch = format!(
            "{head}[… {shown} of {over}{} files shown: the review's patch cap is {cap} bytes; `git diff {base} HEAD` in the lane has the rest]\n",
            files.len()
        );
    }
    Some(yi_types::lane::BranchDiff {
        base: base.chars().take(8).collect(),
        files,
        patch,
        untracked: git(&["ls-files", "--others", "--exclude-standard"])
            .map_or(0, |listed| listed.lines().count()),
    })
}

fn prefix(text: &str, cap: usize) -> &str {
    let end = (0..=cap.min(text.len()))
        .rev()
        .find(|at| text.is_char_boundary(*at))
        .unwrap_or(0);
    text.get(..end).unwrap_or("")
}
