//! Model suggestions per role from an OpenRouter Artificial Analysis snapshot joined to the
//! catalog: a pure score over a cached file, fetched only when the user asks.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};
use yi_types::model::Model;

use crate::refresh::write_atomic;
use crate::request::{ProxyConfig, get_json};

pub const BASE: &str = "https://openrouter.ai/api/v1";
pub const SNAPSHOT: &str = "benchmarks.json";
pub const MIN_REFRESH_MS: u64 = 10 * 60 * 1000;
/// Rank 1 of the daily top 50 earns the whole bonus; a ratio better by more than it always wins.
pub const POPULARITY_CAP: f64 = 0.03;
const TOP: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mix {
    pub uncached: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

impl Mix {
    /// The 7-day Kimi lanes: 97.8% of prompt tokens read from cache, 2% of all tokens output.
    pub const DEFAULT: Mix = Mix {
        uncached: 0.021_56,
        cache_read: 0.958_44,
        cache_write: 0.0,
        output: 0.02,
    };
    const FRESH_READ: Mix = Mix {
        uncached: 0.85,
        cache_read: 0.10,
        cache_write: 0.0,
        output: 0.05,
    };

    pub fn of(uncached: i64, cache_read: i64, cache_write: i64, output: i64) -> Option<Mix> {
        let parts = [uncached, cache_read, cache_write, output].map(|n| n.max(0) as f64);
        let whole: f64 = parts.iter().sum();
        (whole > 0.0).then(|| Mix {
            uncached: parts[0] / whole,
            cache_read: parts[1] / whole,
            cache_write: parts[2] / whole,
            output: parts[3] / whole,
        })
    }

    pub fn line(&self) -> String {
        format!(
            "input {:.1}% cache-read {:.1}% cache-write {:.1}% output {:.1}%",
            self.uncached * 100.0,
            self.cache_read * 100.0,
            self.cache_write * 100.0,
            self.output * 100.0
        )
    }
}

pub struct Role {
    pub name: &'static str,
    pub exponent: f64,
    pub floor: f64,
    pub min_context: u64,
    /// `None` prices the role at the user's own ledger mix.
    pub mix: Option<Mix>,
}

pub const ROLES: &[Role] = &[
    Role {
        name: "primary",
        exponent: 1.0,
        floor: 40.0,
        min_context: 128_000,
        mix: None,
    },
    Role {
        name: "advisor",
        exponent: 2.0,
        floor: 45.0,
        min_context: 0,
        mix: None,
    },
    Role {
        name: "summarizer",
        exponent: 1.0,
        floor: 30.0,
        min_context: 200_000,
        mix: Some(Mix::FRESH_READ),
    },
    Role {
        name: "autoReview",
        exponent: 1.5,
        floor: 35.0,
        min_context: 0,
        mix: Some(Mix::FRESH_READ),
    },
    Role {
        name: "selector",
        exponent: 1.0,
        floor: 25.0,
        min_context: 128_000,
        mix: Some(Mix::FRESH_READ),
    },
];

pub fn role(name: &str) -> Option<&'static Role> {
    ROLES.iter().find(|role| role.name == name)
}

/// Dollars per million tokens, as the catalog stores them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Prices {
    /// A zero cache price means the route does not cache, so those tokens pay full input.
    pub fn blended(&self, mix: &Mix) -> f64 {
        let or_input = |price: f64| if price > 0.0 { price } else { self.input };
        mix.uncached * self.input
            + mix.cache_read * or_input(self.cache_read)
            + mix.cache_write * or_input(self.cache_write)
            + mix.output * self.output
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub intelligence: f64,
    pub prices: Prices,
    pub context: u64,
    /// 1-based place in the daily top 50 by tokens.
    pub rank: Option<usize>,
}

pub fn popularity(rank: Option<usize>) -> f64 {
    let Some(rank) = rank.filter(|rank| (1..=TOP).contains(rank)) else {
        return 1.0;
    };
    1.0 + POPULARITY_CAP * (TOP.saturating_add(1).saturating_sub(rank)) as f64 / TOP as f64
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub tag: &'static str,
    pub id: String,
    pub intelligence: f64,
    pub blended: f64,
    pub popularity: f64,
    pub score: f64,
}

impl Pick {
    pub fn arithmetic(&self, exponent: f64) -> String {
        format!(
            "{:.1}^{exponent} / ${:.4} x pop {:.3} = {:.1}",
            self.intelligence, self.blended, self.popularity, self.score
        )
    }
}

pub struct Ranked {
    pub picks: Vec<Pick>,
    pub below_floor: usize,
    pub short_context: usize,
}

/// High is the most intelligent, medium the best score, low the cheapest above the floor.
pub fn recommend(role: &Role, mix: &Mix, candidates: &[Candidate]) -> Ranked {
    let mut below_floor = 0;
    let mut short_context = 0;
    let mut scored = Vec::new();
    for candidate in candidates {
        if candidate.context < role.min_context {
            short_context += 1;
            continue;
        }
        if candidate.intelligence < role.floor {
            below_floor += 1;
            continue;
        }
        let blended = candidate.prices.blended(mix);
        if blended <= 0.0 {
            continue;
        }
        let popularity = popularity(candidate.rank);
        scored.push(Pick {
            tag: "",
            id: candidate.id.clone(),
            intelligence: candidate.intelligence,
            blended,
            popularity,
            score: candidate.intelligence.powf(role.exponent) / blended * popularity,
        });
    }
    let best = |tag: &'static str, key: &dyn Fn(&Pick) -> (f64, f64)| {
        scored
            .iter()
            .max_by(|left, right| {
                key(left)
                    .partial_cmp(&key(right))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|pick| Pick {
                tag,
                ..pick.clone()
            })
    };
    let picks = [
        best("high", &|pick| (pick.intelligence, -pick.blended)),
        best("medium", &|pick| (pick.score, pick.intelligence)),
        best("low", &|pick| (-pick.blended, pick.intelligence)),
    ]
    .into_iter()
    .flatten()
    .collect();
    Ranked {
        picks,
        below_floor,
        short_context,
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Drops {
    pub scored: usize,
    pub catalog: usize,
    pub no_benchmark: usize,
    pub no_credential: usize,
    pub no_tools: usize,
}

fn undated(slug: &str) -> &str {
    match slug.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => slug,
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
}

fn slug_of(row: &Value) -> Option<&str> {
    ["model_permaslug", "permaslug", "model", "model_id"]
        .iter()
        .find_map(|key| row.get(key).and_then(Value::as_str))
}

/// Daily usage summed per model; the aggregated `other` row is not a model.
fn ranks(rankings: &Value) -> HashMap<String, usize> {
    let mut tokens: HashMap<String, f64> = HashMap::new();
    for row in rankings
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(slug) = slug_of(row).filter(|slug| *slug != "other") else {
            continue;
        };
        let used = number(row.get("total_tokens")).unwrap_or(0.0);
        *tokens.entry(undated(slug).to_owned()).or_default() += used;
    }
    let mut ordered: Vec<(String, f64)> = tokens.into_iter().collect();
    ordered.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    ordered
        .into_iter()
        .take(TOP)
        .enumerate()
        .map(|(index, (slug, _))| (slug, index.saturating_add(1)))
        .collect()
}

pub fn candidates(
    snapshot: &Value,
    models: &[Model],
    routable: &dyn Fn(&str) -> bool,
) -> (Vec<Candidate>, Drops) {
    let intelligence: HashMap<&str, f64> = snapshot
        .pointer("/benchmarks/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                undated(slug_of(row)?),
                number(row.get("intelligence_index"))?,
            ))
        })
        .collect();
    let tools: Option<HashSet<&str>> = snapshot
        .get("tools")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).collect());
    let ranks = snapshot.get("rankings").map(ranks).unwrap_or_default();
    let mut drops = Drops::default();
    let mut out = Vec::new();
    for model in models.iter().filter(|model| model.provider == "openrouter") {
        drops.catalog += 1;
        let Some(score) = intelligence.get(undated(&model.id)) else {
            drops.no_benchmark += 1;
            continue;
        };
        if !routable(&model.provider) {
            drops.no_credential += 1;
            continue;
        }
        if tools
            .as_ref()
            .is_some_and(|ids| !ids.contains(model.id.as_str()))
        {
            drops.no_tools += 1;
            continue;
        }
        let price = |n: &serde_json::Number| n.as_f64().unwrap_or(0.0);
        out.push(Candidate {
            id: format!("{}/{}", model.provider, model.id),
            intelligence: *score,
            prices: Prices {
                input: price(&model.cost.input),
                output: price(&model.cost.output),
                cache_read: price(&model.cost.cache_read),
                cache_write: price(&model.cost.cache_write),
            },
            context: model.context_window,
            rank: ranks.get(undated(&model.id)).copied(),
        });
    }
    drops.scored = out.len();
    (out, drops)
}

pub fn citation(snapshot: &Value) -> String {
    let meta = snapshot.pointer("/benchmarks/meta");
    let field = |key: &str| {
        meta.and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .unwrap_or("-")
    };
    format!(
        "data: {} (CC BY 4.0), as of {}",
        field("citation"),
        field("as_of")
    )
}

pub fn load(dir: &Path) -> Result<Value, String> {
    let path = dir.join(SNAPSHOT);
    let text = std::fs::read(&path).map_err(|_| {
        format!(
            "no benchmark snapshot at {}: run `yi models refresh`",
            path.display()
        )
    })?;
    serde_json::from_slice(&text).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn fetched_ms(snapshot: &Value) -> u64 {
    snapshot
        .get("fetchedMs")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn fetch(url: &str, key: &str, proxy: Option<&ProxyConfig>) -> Result<Value, String> {
    get_json(
        url,
        &[("authorization", format!("Bearer {key}"))],
        proxy,
        crate::refresh::CAP_BYTES,
    )
    .map_err(|error| {
        if error.contains("status code 429") {
            format!("{url}: HTTP 429, OpenRouter's limit is 30 requests a minute and 500 a day")
        } else {
            error
        }
    })
}

/// Three GETs into one snapshot; a failure leaves the last snapshot as it was.
pub fn refresh(
    dir: &Path,
    base: &str,
    key: &str,
    day: &str,
    now_ms: u64,
    proxy: Option<&ProxyConfig>,
) -> Result<usize, String> {
    if let Ok(last) = load(dir) {
        let age = now_ms.saturating_sub(fetched_ms(&last));
        if age < MIN_REFRESH_MS {
            return Err(format!(
                "the snapshot is {}s old; refresh waits {}s between fetches",
                age / 1000,
                MIN_REFRESH_MS / 1000
            ));
        }
    }
    let benchmarks = fetch(
        &format!(
            "{base}/benchmarks?source=artificial-analysis&task_type=intelligence&max_results=100"
        ),
        key,
        proxy,
    )?;
    let count = benchmarks
        .get("data")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if count == 0 {
        return Err("the benchmarks answer held no rows; snapshot left as is".to_owned());
    }
    let rankings = fetch(
        &format!("{base}/datasets/rankings-daily?start_date={day}&end_date={day}"),
        key,
        proxy,
    )?;
    let tools: Vec<Value> = fetch(&format!("{base}/models"), key, proxy)?
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| {
            row.get("supported_parameters")
                .and_then(Value::as_array)
                .is_some_and(|params| params.iter().any(|p| p == "tools"))
        })
        .filter_map(|row| row.get("id").cloned())
        .collect();
    let snapshot = json!({
        "fetchedMs": now_ms,
        "benchmarks": benchmarks,
        "rankings": rankings,
        "tools": tools,
    });
    write_atomic(&dir.join(SNAPSHOT), &snapshot)?;
    Ok(count)
}
