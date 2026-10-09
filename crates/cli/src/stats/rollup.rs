//! `yi stats --since 7d --by model|upstream|day|prices [--top N]`: the sessions directory
//! folded into one table, and the per-route prices its billed dollars imply.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value, json};
use yi_runtime::rollup::{Request, Scan, day_label, now_ms, scan};
use yi_runtime::schedule::Zone;

use super::{Tokens, percentile};

/// Rows a text table shows before it says it stopped; `--json` carries them all.
const ROW_CAP: usize = 40;
/// Replies a route needs before its price is fitted: fewer and four unknowns fit noise.
const MIN_SAMPLES: usize = 30;
/// A component further than this from its catalog price is drift.
const DRIFT: f64 = 0.2;

#[derive(Clone, Copy)]
enum By {
    Model,
    Upstream,
    Day,
}

fn span_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let unit = raw.chars().last()?;
    let digits = raw.strip_suffix(unit)?;
    let scale: u64 = match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        'w' => 604_800_000,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

#[derive(Default)]
struct Group {
    tokens: Tokens,
    reasoning: i64,
    prompts: Vec<u64>,
}

impl Group {
    fn add(&mut self, request: &Request) {
        self.tokens.add(&request.usage, false);
        self.reasoning = self
            .reasoning
            .saturating_add(request.usage.reasoning.unwrap_or(0));
        self.prompts
            .push(u64::try_from(request.prompt()).unwrap_or(0));
    }

    fn reads_per_write(&self) -> Option<f64> {
        (self.tokens.cache_write > 0)
            .then(|| self.tokens.cache_read as f64 / self.tokens.cache_write as f64)
    }

    fn reasoning_share(&self) -> f64 {
        if self.tokens.output > 0 {
            self.reasoning as f64 / self.tokens.output as f64
        } else {
            0.0
        }
    }

    fn json(&self, key: &str) -> Value {
        json!({
            "key": key,
            "requests": self.tokens.turns,
            "billed": self.tokens.cost,
            "billedLowerBound": self.tokens.lower_bound,
            "hitRate": self.tokens.hit_rate(),
            "readsPerWrite": self.reads_per_write(),
            "writeShare": self.tokens.write_share(),
            "reasoningShare": self.reasoning_share(),
            "promptP50": percentile(&self.prompts, 0.5),
            "promptP90": percentile(&self.prompts, 0.9),
        })
    }

    fn row(&self, key: &str) -> String {
        format!(
            "{key:<44} {:>6} {:>9} {:>5.0}% {:>6} {:>5.0}% {:>5.0}% {:>7} {:>7}",
            self.tokens.turns,
            yi_types::message::fmt_cost(self.tokens.cost, self.tokens.lower_bound),
            self.tokens.hit_rate() * 100.0,
            self.reads_per_write()
                .map_or_else(|| "-".to_owned(), |ratio| format!("{ratio:.1}")),
            self.tokens.write_share() * 100.0,
            self.reasoning_share() * 100.0,
            percentile(&self.prompts, 0.5),
            percentile(&self.prompts, 0.9),
        )
    }
}

fn route(request: &Request) -> (String, String) {
    (
        format!("{}/{}", request.provider, request.model),
        request
            .upstream
            .clone()
            .unwrap_or_else(|| "direct".to_owned()),
    )
}

fn grouped(scan: &Scan, by: By, zone: &Zone) -> Vec<(String, Group)> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for request in &scan.requests {
        let key = match by {
            By::Model => route(request).0,
            By::Upstream => route(request).1,
            By::Day => day_label(zone, request.at_ms),
        };
        groups.entry(key).or_default().add(request);
    }
    let mut rows: Vec<(String, Group)> = groups.into_iter().collect();
    for (_, group) in &mut rows {
        group.prompts.sort_unstable();
    }
    if !matches!(by, By::Day) {
        rows.sort_by(|a, b| b.1.tokens.cost.total_cmp(&a.1.tokens.cost));
    }
    rows
}

struct Costly {
    session: String,
    child: bool,
    model: String,
    group: Group,
}

fn costliest(scan: &Scan, top: usize) -> Vec<Costly> {
    let mut sessions: BTreeMap<&str, Costly> = BTreeMap::new();
    for request in &scan.requests {
        let row = sessions.entry(&request.session).or_insert_with(|| Costly {
            session: request.session.clone(),
            child: request.child,
            model: route(request).0,
            group: Group::default(),
        });
        row.group.add(request);
    }
    let mut rows: Vec<Costly> = sessions.into_values().collect();
    rows.sort_by(|a, b| b.group.tokens.cost.total_cmp(&a.group.tokens.cost));
    rows.truncate(top);
    for row in &mut rows {
        row.group.prompts.sort_unstable();
    }
    rows
}

/// One pass of a pivoting elimination over augmented rows `[coefficients.., rhs]`; `None` when
/// the system is singular, which is what a column that never varies looks like.
fn solve(mut rows: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    if rows.is_empty() {
        return Some(Vec::new());
    }
    let lead = |row: &Vec<f64>| row.first().copied().unwrap_or(0.0).abs();
    let at = rows
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| lead(a).total_cmp(&lead(b)))
        .map(|(at, _)| at)?;
    let pivot = rows.swap_remove(at);
    let head = *pivot.first()?;
    if head.abs() < 1e-12 {
        return None;
    }
    let reduced = rows
        .into_iter()
        .map(|row| {
            let factor = row.first().copied().unwrap_or(0.0) / head;
            row.iter()
                .zip(&pivot)
                .skip(1)
                .map(|(a, p)| a - factor * p)
                .collect()
        })
        .collect();
    let tail = solve(reduced)?;
    let known: f64 = pivot.iter().skip(1).zip(&tail).map(|(p, x)| p * x).sum();
    let first = (pivot.last()? - known) / head;
    Some(std::iter::once(first).chain(tail).collect())
}

/// Dollars per million tokens for [input, output, cacheRead, cacheWrite] and the root-mean-square
/// miss as a fraction of the billed total. A component with no tokens in any sample is `None`.
fn fit(samples: &[&Request]) -> Option<([Option<f64>; 4], f64)> {
    let tokens = |request: &Request| {
        let usage = &request.usage;
        [
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
        ]
        .map(|count| count.max(0) as f64 / 1e6)
    };
    let rows: Vec<([f64; 4], f64)> = samples
        .iter()
        .map(|request| (tokens(request), request.billed()))
        .collect();
    let live: Vec<usize> = (0..4)
        .filter(|column| {
            rows.iter()
                .any(|(x, _)| x.get(*column).is_some_and(|v| *v > 0.0))
        })
        .collect();
    let at = |x: &[f64; 4], column: usize| x.get(column).copied().unwrap_or(0.0);
    let normal: Vec<Vec<f64>> = live
        .iter()
        .map(|row| {
            let mut line: Vec<f64> = live
                .iter()
                .map(|column| rows.iter().map(|(x, _)| at(x, *row) * at(x, *column)).sum())
                .collect();
            line.push(rows.iter().map(|(x, y)| at(x, *row) * y).sum());
            line
        })
        .collect();
    let solved = solve(normal)?;
    let mut prices = [None; 4];
    for (column, price) in live.iter().zip(&solved) {
        if let Some(slot) = prices.get_mut(*column) {
            *slot = Some(*price);
        }
    }
    let (miss, billed) = rows.iter().fold((0.0, 0.0), |(miss, billed), (x, y)| {
        let predicted: f64 = prices
            .iter()
            .enumerate()
            .map(|(column, price)| price.unwrap_or(0.0) * at(x, column))
            .sum();
        (miss + (predicted - y).powi(2), billed + y * y)
    });
    (billed > 0.0).then(|| (prices, (miss / billed).sqrt()))
}

fn catalog(provider: &str, model: &str) -> Option<[f64; 4]> {
    let cost = yi_runtime::resolve_model(provider, model)?.cost;
    [cost.input, cost.output, cost.cache_read, cost.cache_write]
        .map(|price| price.as_f64())
        .into_iter()
        .collect::<Option<Vec<f64>>>()?
        .try_into()
        .ok()
}

struct Observed {
    route: String,
    upstream: String,
    samples: usize,
    prices: [Option<f64>; 4],
    residual: f64,
    catalog: Option<[f64; 4]>,
}

impl Observed {
    /// Components whose observed price sits further than [`DRIFT`] from the catalog's.
    fn drifted(&self) -> [bool; 4] {
        let mut flags = [false; 4];
        let Some(catalog) = self.catalog else {
            return flags;
        };
        for ((flag, seen), listed) in flags.iter_mut().zip(self.prices).zip(catalog) {
            if let Some(seen) = seen {
                *flag = if listed > 0.0 {
                    (seen / listed - 1.0).abs() > DRIFT
                } else {
                    seen.abs() > 1e-9
                };
            }
        }
        flags
    }

    fn line(&self) -> String {
        const NAMES: [&str; 4] = ["in", "out", "cacheR", "cacheW"];
        let flags = self.drifted();
        let parts: Vec<String> = (0..4)
            .map(|column| {
                let seen = self.prices.get(column).copied().flatten();
                let listed = self.catalog.and_then(|c| c.get(column).copied());
                format!(
                    "{} {} ({}){}",
                    NAMES.get(column).copied().unwrap_or(""),
                    seen.map_or_else(|| "-".to_owned(), |price| format!("{price:.3}")),
                    listed.map_or_else(|| "?".to_owned(), |price| format!("{price:.3}")),
                    if flags.get(column).copied().unwrap_or(false) {
                        "!"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        format!(
            "{} via {}  n={} resid {:.1}%  {}{}",
            self.route,
            self.upstream,
            self.samples,
            self.residual * 100.0,
            parts.join("  "),
            if self.catalog.is_none() {
                "  [no catalog entry: nothing to compare]"
            } else {
                ""
            }
        )
    }
}

/// Every (model, upstream) route with enough priced replies, fitted; plus the count left unfit.
fn observed(scan: &Scan) -> (Vec<Observed>, usize) {
    let mut routes: BTreeMap<(String, String), Vec<&Request>> = BTreeMap::new();
    for request in scan.requests.iter().filter(|r| !r.usage.unknown) {
        routes.entry(route(request)).or_default().push(request);
    }
    let mut fits = Vec::new();
    let mut unfit = 0_usize;
    for ((name, upstream), samples) in routes {
        let first = samples.first().copied();
        let fitted = first
            .filter(|_| samples.len() >= MIN_SAMPLES)
            .and_then(|first| {
                fit(&samples).map(|(prices, residual)| Observed {
                    route: name.clone(),
                    upstream: upstream.clone(),
                    samples: samples.len(),
                    prices,
                    residual,
                    catalog: catalog(&first.provider, &first.model),
                })
            });
        match fitted {
            Some(row) => fits.push(row),
            None => unfit = unfit.saturating_add(1),
        }
    }
    fits.sort_by(|a, b| b.samples.cmp(&a.samples));
    (fits, unfit)
}

/// The prices view: every route's fit, its drift from the catalog, and the routes left out.
fn prices_view(scanned: &Scan, window: &str, skipped: Option<String>, json_out: bool) -> i32 {
    let (fits, unfit) = observed(scanned);
    let unfit_note = format!(
        "[{unfit} route(s) not fitted: fewer than {MIN_SAMPLES} priced replies (MIN_SAMPLES), or a column that never varies; widen --since to include more]"
    );
    if json_out {
        let rows: Vec<Value> = fits
            .iter()
            .map(|row| {
                json!({
                    "model": row.route, "upstream": row.upstream, "samples": row.samples,
                    "observed": row.prices, "catalog": row.catalog,
                    "residual": row.residual, "drifted": row.drifted(),
                })
            })
            .collect();
        println!(
            "{}",
            json!({"since": window, "prices": rows, "unfit": unfit, "skipped": skipped})
        );
        return 0;
    }
    println!(
        "observed prices, $ per M tokens, observed (catalog), ! past {:.0}% drift; {window}",
        DRIFT * 100.0
    );
    for row in fits.iter().take(ROW_CAP) {
        println!("{}", row.line());
    }
    if fits.len() > ROW_CAP {
        println!(
            "[showing {ROW_CAP} of {} routes (ROW_CAP); `yi stats --by prices --json` lists all]",
            fits.len()
        );
    }
    if unfit > 0 {
        println!("{unfit_note}");
    }
    if let Some(note) = skipped {
        println!("{note}");
    }
    0
}

pub fn run(flags: &[(String, String)], dir: &std::path::Path, json_out: bool) -> i32 {
    let flag = |name: &str| {
        flags
            .iter()
            .rev()
            .find(|(given, _)| given == name)
            .map(|(_, value)| value.as_str())
    };
    let by = match flag("by") {
        None | Some("model" | "prices") => By::Model,
        Some("upstream") => By::Upstream,
        Some("day") => By::Day,
        Some(other) => {
            eprintln!("error: --by {other} is not one of model, upstream, day, prices");
            return 2;
        }
    };
    let now = now_ms();
    let since = match flag("since") {
        None => None,
        Some(raw) => match span_ms(raw) {
            Some(span) => Some(now.saturating_sub(span)),
            None => {
                eprintln!("error: --since {raw} is not a span like 90m, 12h, 7d or 2w");
                return 2;
            }
        },
    };
    let top = match flag("top").map(str::parse::<usize>) {
        None => None,
        Some(Ok(top)) => Some(top),
        Some(Err(_)) => {
            eprintln!("error: --top needs a number of sessions");
            return 2;
        }
    };
    let scanned = scan(dir, since.unwrap_or(0));
    let window = flag("since").unwrap_or("all time");
    let skipped = (scanned.unreadable > 0 || scanned.bad_lines > 0).then(|| {
        format!(
            "[skipped {} unreadable file(s) and {} unparseable line(s) of {} file(s) read; every figure leaves them out]",
            scanned.unreadable, scanned.bad_lines, scanned.files
        )
    });
    if flag("by") == Some("prices") {
        return prices_view(&scanned, window, skipped, json_out);
    }
    let zone = Zone::local();
    let rows = grouped(&scanned, by, &zone);
    let sessions: HashSet<&str> = scanned
        .requests
        .iter()
        .map(|r| r.session.as_str())
        .collect();
    let costly = top.map(|top| costliest(&scanned, top));
    if json_out {
        let table: Vec<Value> = rows.iter().map(|(key, group)| group.json(key)).collect();
        let costly: Option<Vec<Value>> = costly.as_ref().map(|rows| {
            rows.iter()
                .map(|row| {
                    let mut value = row.group.json(&row.session);
                    value["child"] = json!(row.child);
                    value["model"] = json!(row.model);
                    value
                })
                .collect()
        });
        println!(
            "{}",
            json!({
                "since": window, "files": scanned.files, "sessions": sessions.len(),
                "requests": scanned.requests.len(), "rows": table, "top": costly,
                "unreadable": scanned.unreadable, "badLines": scanned.bad_lines,
            })
        );
        return 0;
    }
    println!(
        "{window}: {} request(s) in {} session(s), {} file(s)",
        scanned.requests.len(),
        sessions.len(),
        scanned.files
    );
    println!(
        "{:<44} {:>6} {:>9} {:>6} {:>6} {:>6} {:>6} {:>7} {:>7}",
        "key", "reqs", "billed", "hit", "r/w", "write", "reason", "p50", "p90"
    );
    for (key, group) in rows.iter().take(ROW_CAP) {
        println!("{}", group.row(key));
    }
    if rows.len() > ROW_CAP {
        println!(
            "[showing {ROW_CAP} of {} rows (ROW_CAP); narrow with --since, or `--json` lists all]",
            rows.len()
        );
    }
    if let Some(costly) = costly {
        println!(
            "costliest {} session(s), by the model of the first reply:",
            costly.len()
        );
        for row in &costly {
            let kind = if row.child { "child" } else { "root" };
            println!(
                "  {}  {}",
                row.session,
                row.group.row(&format!("{kind} {}", row.model))
            );
        }
    }
    if let Some(note) = skipped {
        println!("{note}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priced(prices: [Option<f64>; 4], catalog: Option<[f64; 4]>) -> Observed {
        Observed {
            route: String::new(),
            upstream: String::new(),
            samples: MIN_SAMPLES,
            prices,
            residual: 0.0,
            catalog,
        }
    }

    /// Dies when a component 20% off its catalog price goes unflagged, or one just inside is.
    #[test]
    fn drift_is_flagged_past_twenty_percent_and_not_inside_it() {
        let seen = [Some(0.12), Some(0.59), Some(0.0), None];
        let listed = Some([0.15, 0.50, 0.0, 0.0]);
        // 0.12 / 0.15 is exactly -20%: not past it; 0.59 / 0.50 is +18%.
        assert_eq!(priced(seen, listed).drifted(), [false, false, false, false]);
        let seen = [Some(0.119), Some(0.61), Some(0.01), None];
        assert_eq!(priced(seen, listed).drifted(), [true, true, true, false]);
        assert_eq!(priced(seen, None).drifted(), [false; 4]);
    }

    /// Dies when `--since` accepts a span it cannot read, or overflows on a huge one.
    #[test]
    fn a_span_needs_a_number_and_a_known_unit() {
        assert_eq!(span_ms("90m"), Some(5_400_000));
        assert_eq!(span_ms("2w"), Some(1_209_600_000));
        for bad in ["", "d", "7", "7x", "-1d", "1.5d", "99999999999999999999d"] {
            assert_eq!(span_ms(bad), None, "{bad}");
        }
    }

    /// Dies when a pair of columns that always move together is "fitted": two prices cannot be
    /// told apart there, so the honest answer is no fit.
    #[test]
    fn collinear_columns_give_no_fit() {
        assert_eq!(solve(vec![vec![1.0, 2.0, 3.0], vec![2.0, 4.0, 6.0]]), None);
        assert_eq!(
            solve(vec![vec![2.0, 1.0, 5.0], vec![1.0, 3.0, 10.0]]),
            Some(vec![1.0, 3.0])
        );
    }
}
