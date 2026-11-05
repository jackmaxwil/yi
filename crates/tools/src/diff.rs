use std::path::Path;

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPatch(String);

impl GitPatch {
    pub fn from_text(text: String) -> Self {
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Counts exclude the `--- a/` and `+++ b/` header pair, which shares the
    /// sign column with change rows but is not one.
    pub fn stats(&self) -> (u64, u64) {
        let mut added: u64 = 0;
        let mut removed: u64 = 0;
        for line in self.0.lines() {
            if line.starts_with("+++ ") || line.starts_with("--- ") {
                continue;
            }
            match line.as_bytes().first() {
                Some(b'+') => added = added.saturating_add(1),
                Some(b'-') => removed = removed.saturating_add(1),
                _ => {}
            }
        }
        (added, removed)
    }
}

/// The `patch` / `added` / `removed` keys an edit or write result carries so the transcript
/// can render a diff body (§7). Counts cover the whole change even when the text is capped.
pub fn patch_details(patch: &GitPatch) -> Value {
    let (added, removed) = patch.stats();
    json!({
        "patch": crate::tool::detail_text(patch.as_str()),
        "added": added,
        "removed": removed,
    })
}

const CONTEXT: usize = 3;

// ponytail: the LCS table is quadratic, so a change region wider than this
// degrades to one replace-everything hunk instead of allocating gigabytes.
const MAX_REGION_LINES: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Remove,
    Add,
}

/// One line diff of a (before, after) pair: built once, read by every consumer of an edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditScript {
    steps: Vec<(Op, String)>,
    after_lines: u64,
}

impl EditScript {
    /// Invariant: one LCS per pair. The table is quadratic, so a second build for the same
    /// edit is the slowest thing an edit does.
    pub fn between(pre: &str, post: &str) -> Self {
        let before = split_lines(pre);
        let after = split_lines(post);
        Self {
            steps: edit_script(&before, &after)
                .into_iter()
                .map(|(op, line)| (op, line.to_owned()))
                .collect(),
            after_lines: u64::try_from(after.len()).unwrap_or(u64::MAX),
        }
    }

    fn borrowed(&self) -> Vec<(Op, &str)> {
        self.steps
            .iter()
            .map(|(op, line)| (*op, line.as_str()))
            .collect()
    }

    pub fn is_unchanged(&self) -> bool {
        self.steps.iter().all(|(op, _)| *op == Op::Keep)
    }

    /// Absolute paths in the headers: the permission display and ACP `diff.patch`
    /// both read them.
    pub fn patch(&self, path: &Path) -> GitPatch {
        if self.is_unchanged() {
            return GitPatch(String::new());
        }
        let hunks = hunks(&self.borrowed());
        let name = path.display();
        let mut out = format!("--- a/{name}\n+++ b/{name}\n");
        for hunk in hunks {
            out.push_str(&hunk);
        }
        GitPatch(out)
    }

    /// Line numbers in the post text that the pre text lacks, one per added or replaced line.
    /// A deletion reports the line that closed over it, so a removal-only hunk still has one.
    pub fn changed_after_lines(&self) -> Vec<u64> {
        let mut changed = Vec::new();
        let mut after_line: u64 = 1;
        for (op, _) in &self.steps {
            match op {
                Op::Keep => after_line = after_line.saturating_add(1),
                Op::Add => {
                    changed.push(after_line);
                    after_line = after_line.saturating_add(1);
                }
                Op::Remove => changed.push(after_line),
            }
        }
        changed.dedup();
        changed
    }

    /// The post lines a rendered patch shows: every changed line and its context rows.
    pub fn shown_after_lines(&self) -> Vec<u64> {
        let context = u64::try_from(CONTEXT).unwrap_or(u64::MAX);
        let mut shown: Vec<u64> = Vec::new();
        for changed in self.changed_after_lines() {
            let lo = changed.saturating_sub(context).max(1);
            let hi = changed.saturating_add(context).min(self.after_lines);
            let start = shown
                .last()
                .map_or(lo, |last| lo.max(last.saturating_add(1)));
            shown.extend(start..=hi);
        }
        shown
    }

    /// Where each line of the pre text sits in the post text: `Some(n)` for a line whose text
    /// is unchanged, `None` for one that was removed or rewritten.
    pub fn line_map(&self) -> Vec<Option<u64>> {
        let mut map: Vec<Option<u64>> = Vec::with_capacity(self.steps.len());
        let mut after_line: u64 = 1;
        for (op, _) in &self.steps {
            match op {
                Op::Keep => {
                    map.push(Some(after_line));
                    after_line = after_line.saturating_add(1);
                }
                Op::Remove => map.push(None),
                Op::Add => after_line = after_line.saturating_add(1),
            }
        }
        map
    }
}

pub fn patch(pre: &str, post: &str, path: &Path) -> GitPatch {
    if pre == post {
        return GitPatch(String::new());
    }
    EditScript::between(pre, post).patch(path)
}

pub fn shown_after_lines(pre: &str, post: &str) -> Vec<u64> {
    EditScript::between(pre, post).shown_after_lines()
}

pub fn line_map(pre: &str, post: &str) -> Vec<Option<u64>> {
    EditScript::between(pre, post).line_map()
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn hunks(script: &[(Op, &str)]) -> Vec<String> {
    let mut before_no = Vec::with_capacity(script.len());
    let mut after_no = Vec::with_capacity(script.len());
    let (mut before_line, mut after_line) = (1_usize, 1_usize);
    for (op, _) in script {
        before_no.push(before_line);
        after_no.push(after_line);
        match op {
            Op::Keep => {
                before_line = before_line.saturating_add(1);
                after_line = after_line.saturating_add(1);
            }
            Op::Remove => before_line = before_line.saturating_add(1),
            Op::Add => after_line = after_line.saturating_add(1),
        }
    }
    let mut rendered = Vec::new();
    let mut index = 0;
    while index < script.len() {
        if script.get(index).map(|(op, _)| *op) == Some(Op::Keep) {
            index = index.saturating_add(1);
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let mut end = index;
        let mut keeps = 0_usize;
        while end < script.len() {
            match script.get(end).map(|(op, _)| *op) {
                Some(Op::Keep) => {
                    keeps = keeps.saturating_add(1);
                    if keeps > CONTEXT.saturating_mul(2) {
                        break;
                    }
                }
                Some(_) => keeps = 0,
                None => break,
            }
            end = end.saturating_add(1);
        }
        let end = end.saturating_sub(keeps.saturating_sub(CONTEXT.min(keeps)));
        let Some(slice) = script.get(start..end) else {
            break;
        };
        rendered.push(render_hunk(
            slice,
            before_no.get(start).copied().unwrap_or(1),
            after_no.get(start).copied().unwrap_or(1),
        ));
        index = end;
    }
    rendered
}

fn render_hunk(slice: &[(Op, &str)], before_start: usize, after_start: usize) -> String {
    let before_count = slice
        .iter()
        .filter(|(op, _)| matches!(op, Op::Keep | Op::Remove))
        .count();
    let after_count = slice
        .iter()
        .filter(|(op, _)| matches!(op, Op::Keep | Op::Add))
        .count();
    let mut out = format!("@@ -{before_start},{before_count} +{after_start},{after_count} @@\n");
    for (op, text) in slice {
        let marker = match op {
            Op::Keep => ' ',
            Op::Remove => '-',
            Op::Add => '+',
        };
        out.push(marker);
        out.push_str(text);
        out.push('\n');
    }
    out
}

fn edit_script<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<(Op, &'a str)> {
    let prefix = common_prefix(before, after);
    let before_tail = before.get(prefix..).unwrap_or_default();
    let after_tail = after.get(prefix..).unwrap_or_default();
    let suffix = common_suffix(before_tail, after_tail);
    let mut script: Vec<(Op, &str)> = before
        .iter()
        .take(prefix)
        .map(|line| (Op::Keep, *line))
        .collect();
    let before_mid = before_tail
        .get(..before_tail.len().saturating_sub(suffix))
        .unwrap_or_default();
    let after_mid = after_tail
        .get(..after_tail.len().saturating_sub(suffix))
        .unwrap_or_default();
    if before_mid.len() > MAX_REGION_LINES || after_mid.len() > MAX_REGION_LINES {
        script.extend(before_mid.iter().map(|line| (Op::Remove, *line)));
        script.extend(after_mid.iter().map(|line| (Op::Add, *line)));
    } else {
        script.extend(lcs_script(before_mid, after_mid));
    }
    script.extend(
        before
            .iter()
            .skip(before.len().saturating_sub(suffix))
            .map(|line| (Op::Keep, *line)),
    );
    script
}

fn common_prefix(before: &[&str], after: &[&str]) -> usize {
    before
        .iter()
        .zip(after.iter())
        .take_while(|(left, right)| left == right)
        .count()
}

fn common_suffix(before: &[&str], after: &[&str]) -> usize {
    before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

fn lcs_script<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<(Op, &'a str)> {
    let columns = after.len().saturating_add(1);
    let mut table = vec![0_u32; before.len().saturating_add(1).saturating_mul(columns)];
    let cell = |table: &[u32], row: usize, column: usize| -> u32 {
        table
            .get(row.saturating_mul(columns).saturating_add(column))
            .copied()
            .unwrap_or(0)
    };
    for row in (0..before.len()).rev() {
        for column in (0..after.len()).rev() {
            let value = if before.get(row) == after.get(column) {
                cell(&table, row.saturating_add(1), column.saturating_add(1)).saturating_add(1)
            } else {
                cell(&table, row.saturating_add(1), column).max(cell(
                    &table,
                    row,
                    column.saturating_add(1),
                ))
            };
            if let Some(slot) = table.get_mut(row.saturating_mul(columns).saturating_add(column)) {
                *slot = value;
            }
        }
    }
    let mut script = Vec::new();
    let (mut row, mut column) = (0_usize, 0_usize);
    while row < before.len() && column < after.len() {
        if before.get(row) == after.get(column) {
            script.push((Op::Keep, *before.get(row).unwrap_or(&"")));
            row = row.saturating_add(1);
            column = column.saturating_add(1);
        } else if cell(&table, row.saturating_add(1), column)
            >= cell(&table, row, column.saturating_add(1))
        {
            script.push((Op::Remove, *before.get(row).unwrap_or(&"")));
            row = row.saturating_add(1);
        } else {
            script.push((Op::Add, *after.get(column).unwrap_or(&"")));
            column = column.saturating_add(1);
        }
    }
    script.extend(before.iter().skip(row).map(|line| (Op::Remove, *line)));
    script.extend(after.iter().skip(column).map(|line| (Op::Add, *line)));
    script
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::EditScript;

    const BEFORE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
    const AFTER: &str = "one\nTWO\nthree\nfour\nsix\nseven\neight\n";

    #[test]
    fn one_script_renders_the_patch_and_each_line_view() {
        let script = EditScript::between(BEFORE, AFTER);
        let path = Path::new("/tmp/f.rs");
        assert_eq!(
            script.patch(path).as_str(),
            concat!(
                "--- a//tmp/f.rs\n",
                "+++ b//tmp/f.rs\n",
                "@@ -1,7 +1,7 @@\n",
                " one\n",
                "-two\n",
                "+TWO\n",
                " three\n",
                " four\n",
                "-five\n",
                " six\n",
                " seven\n",
                "+eight\n",
            )
        );
        assert_eq!(script.changed_after_lines(), vec![2, 5, 7]);
        assert_eq!(script.shown_after_lines(), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            script.line_map(),
            vec![Some(1), None, Some(3), Some(4), None, Some(5), Some(6)]
        );
    }

    #[test]
    fn an_unchanged_pair_is_no_patch_and_no_map() {
        let script = EditScript::between(BEFORE, BEFORE);
        assert!(script.is_unchanged());
        assert!(script.patch(Path::new("/tmp/f.rs")).as_str().is_empty());
        assert!(script.changed_after_lines().is_empty());
        assert_eq!(
            script.line_map(),
            vec![
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(6),
                Some(7)
            ]
        );
    }

    #[test]
    fn the_changed_lines_are_post_lines_and_stay_inside_the_text() {
        let script = EditScript::between(
            "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n",
            "a\nc\nd\ne\nf\ng\nh\ni\nj\n",
        );
        assert_eq!(script.changed_after_lines(), vec![2]);
        assert!(script.shown_after_lines().iter().all(|line| *line <= 9));
        assert_eq!(script.line_map().len(), 10);
    }
}
