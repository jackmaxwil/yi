use std::path::{Path, PathBuf};

use globset::GlobMatcher;

struct Rule {
    base: PathBuf,
    matcher: GlobMatcher,
    dir_only: bool,
    negate: bool,
}

/// Appended as a walk descends, never popped: a rule applies only under the
/// directory that declared it, and the `base` prefix check is what keeps a
/// sibling's rules from leaking across.
#[derive(Default)]
pub struct Ignore {
    rules: Vec<Rule>,
}

impl Ignore {
    /// A missing or unreadable file leaves the set unchanged.
    pub fn push_dir(&mut self, dir: &Path) {
        let Ok(text) = std::fs::read_to_string(dir.join(".gitignore")) else {
            return;
        };
        for line in text.lines() {
            if let Some(rule) = parse_line(line, dir) {
                self.rules.push(rule);
            }
        }
    }

    /// Last matching rule wins, as git specifies. `.git` is always ignored.
    // ponytail: linear scan over every gathered rule; a prefix-indexed set if
    // a repo ever carries enough .gitignore files to show up in a profile.
    pub fn ignored(&self, path: &Path, is_dir: bool) -> bool {
        if path.file_name().is_some_and(|name| name == ".git") {
            return true;
        }
        let mut ignored = false;
        for rule in &self.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&rule.base) else {
                continue;
            };
            if rule.matcher.is_match(relative) {
                ignored = !rule.negate;
            }
        }
        ignored
    }
}

fn parse_line(line: &str, dir: &Path) -> Option<Rule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negate, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let (dir_only, line) = match line.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    if line.is_empty() {
        return None;
    }
    // A pattern with an interior slash is anchored to the declaring
    // directory; a bare name matches at any depth below it.
    let anchored = line.trim_start_matches('/').contains('/');
    let pattern = if anchored {
        line.trim_start_matches('/').to_owned()
    } else {
        format!("**/{}", line.trim_start_matches('/'))
    };
    let matcher = globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .ok()?
        .compile_matcher();
    Some(Rule {
        base: dir.to_path_buf(),
        matcher,
        dir_only,
        negate,
    })
}
