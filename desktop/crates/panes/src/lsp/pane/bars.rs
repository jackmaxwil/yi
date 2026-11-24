//! The pane's bars: the servers' count line, the problems' heading and the rule at the foot.

use std::fmt;

use rune::{El, Icon, Tx, el, text};
use yd_core::command::table::Run;
use yd_core::command::tools::lsp::RESTART;
use yd_core::feed::Table;
use yd_core::row::{Dot, Rows, Value};
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::{footer as foot, header, lead};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::line_icon;
use yd_ui::menu::command_box;
use yd_ui::theme::look::{LABEL, META, colour};
use yd_ui::win::Win;

static RELOAD: Icon = line_icon!(
    "reload",
    r#"<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7"/>"#
);

/// The sentence the pane ends with, stating its rule.
const RULE: &str = "The agent sees the same problems you do.";

fn rows(table: &Table) -> impl Iterator<Item = &yd_core::row::Row> {
    (0..table.len()).filter_map(|at| table.get(at))
}

/// How many problems of each severity a list holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Summary {
    errors: usize,
    warnings: usize,
    info: usize,
    hints: usize,
}

impl Summary {
    pub(super) fn of(problems: &Table) -> Summary {
        let mut sum = Summary::default();
        for row in rows(problems) {
            let count = match row.words("severity") {
                Some("Error") => &mut sum.errors,
                Some("Warn") => &mut sum.warnings,
                Some("Info") => &mut sum.info,
                _ => &mut sum.hints,
            };
            *count = count.saturating_add(1);
        }
        sum
    }
}

impl fmt::Display for Summary {
    /// `1 error · 2 warnings · 1 hint`: the severities there are, in that order.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kinds = [
            (self.errors, "error"),
            (self.warnings, "warning"),
            (self.info, "info"),
            (self.hints, "hint"),
        ];
        let said: Vec<String> = kinds
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, word)| {
                format!(
                    "{n} {word}{}",
                    if *n == 1 || *word == "info" { "" } else { "s" }
                )
            })
            .collect();
        if said.is_empty() {
            out.write_str("No problems")
        } else {
            out.write_str(&said.join(" · "))
        }
    }
}

/// The requests the busiest server has answered this turn, read from its state line.
pub(super) fn requests(servers: &Table) -> Option<u64> {
    let answered = |words: &str| {
        let (_, after) = words.split_once("answered ")?;
        after.split_whitespace().next()?.parse::<u64>().ok()
    };
    rows(servers)
        .filter_map(|row| answered(row.words("status")?))
        .max()
}

/// `2 running`, then how many are off, and the way to restart them all.
pub(super) fn servers_head(tokens: &Tokens, key: &str, servers: &Table) -> El {
    let running = rows(servers)
        .filter(|row| row.get("state") == Some(&Value::Dot(Dot::Run)))
        .count();
    let off = servers.len().saturating_sub(running);
    let dim = |said: String| text(said).font(LABEL).color(colour(tokens.dim));
    let off = (off > 0).then(|| dim(format!("{off} off · started by plugins")));
    let all = chip(tokens, Chip::Plain, "Restart all", Some(&RELOAD), false)
        .key(format!("{key}.restart-all"))
        .on_press_tx(|win: &mut Win, tx: &mut Tx| {
            command_box::run(win, tx, Run::new(RESTART));
        });
    header(
        tokens,
        [lead(tokens, &format!("{running} running"))]
            .into_iter()
            .chain(off)
            .collect(),
        vec![all],
    )
    .key(format!("{key}.pane-header"))
}

/// `Problems`, how many of each severity, and the scope they are listed for.
pub(super) fn problems_head(tokens: &Tokens, key: &str, problems: &Table) -> El {
    let summary = text(Summary::of(problems).to_string())
        .font(LABEL)
        .color(colour(tokens.dim));
    let scope = chip(tokens, Chip::Plain, "This thread", None, true);
    header(tokens, vec![lead(tokens, "Problems"), summary], vec![scope]).key(key)
}

/// The rule at the foot: an accent dot, then the sentence.
pub(super) fn footer(tokens: &Tokens, key: &str) -> El {
    let dot = el().size(6.0, 6.0).radius(3.0).fill(colour(tokens.accent));
    let rule = text(RULE).font(META).color(colour(tokens.faint));
    foot(tokens, vec![dot, rule], Vec::new()).key(format!("{key}.footer"))
}
