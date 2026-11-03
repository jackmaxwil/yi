//! The recommender over hand-priced candidates, the recorded 2026-09-05 OpenRouter models list,
//! and a loopback OpenRouter for the refresh; no network.

use std::error::Error;

use serde_json::{Value, json};
use yi_ai::recommend::{
    Candidate, MIN_REFRESH_MS, Mix, POPULARITY_CAP, Prices, SNAPSHOT, candidates, citation, load,
    popularity, recommend, refresh, role,
};

use crate::common::model;
use crate::loopback_server::serve;
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const OPENROUTER_LIST: &str = include_str!("fixtures/openrouter_models_2026-09-05.json");

fn candidate(id: &str, intelligence: f64, prices: [f64; 3], context: u64) -> Candidate {
    Candidate {
        id: id.to_owned(),
        intelligence,
        prices: Prices {
            input: prices[0],
            output: prices[1],
            cache_read: prices[2],
            cache_write: 0.0,
        },
        context,
        rank: None,
    }
}

fn field() -> Vec<Candidate> {
    vec![
        candidate("a", 60.0, [1.0, 4.0, 0.1], 256_000),
        candidate("b", 50.0, [0.3, 1.2, 0.03], 256_000),
        candidate("c", 35.0, [0.05, 0.1, 0.01], 1_000_000),
    ]
}

fn picks(name: &str, mix: &Mix, field: &[Candidate]) -> Result<Vec<(String, String, f64)>, String> {
    let role = role(name).ok_or(format!("no role {name}"))?;
    Ok(recommend(role, mix, field)
        .picks
        .into_iter()
        .map(|pick| {
            (
                pick.tag.to_owned(),
                pick.id,
                (pick.score * 1000.0).round() / 1000.0,
            )
        })
        .collect())
}

/// Each role's high, medium and low and the score behind each, worked by hand from the
/// role's exponent, floors and token mix.
#[test]
fn each_role_picks_its_hand_computed_high_medium_and_low() -> TestResult {
    let tagged = |rows: &[(&str, &str, f64)]| -> Vec<(String, String, f64)> {
        rows.iter()
            .map(|(t, id, s)| ((*t).to_owned(), (*id).to_owned(), *s))
            .collect()
    };
    let cases = [
        (
            "primary",
            Mix::DEFAULT,
            tagged(&[
                ("high", "a", 303.945),
                ("medium", "b", 844.292),
                ("low", "b", 844.292),
            ]),
        ),
        (
            "advisor",
            Mix::DEFAULT,
            tagged(&[
                ("high", "a", 18236.713),
                ("medium", "b", 42214.612),
                ("low", "b", 42214.612),
            ]),
        ),
        (
            "summarizer",
            Mix {
                uncached: 0.85,
                cache_read: 0.10,
                cache_write: 0.0,
                output: 0.05,
            },
            tagged(&[
                ("high", "a", 56.604),
                ("medium", "c", 721.649),
                ("low", "c", 721.649),
            ]),
        ),
        (
            "autoReview",
            Mix {
                uncached: 0.85,
                cache_read: 0.10,
                cache_write: 0.0,
                output: 0.05,
            },
            tagged(&[
                ("high", "a", 438.451),
                ("medium", "c", 4269.336),
                ("low", "c", 4269.336),
            ]),
        ),
    ];
    for (name, mix, want) in cases {
        assert_eq!(picks(name, &mix, &field())?, want, "{name}");
    }
    Ok(())
}

/// The daily top spot is worth at most the cap, so a model priced at a clearly better ratio
/// still takes medium from the most used one.
#[test]
fn popularity_cannot_flip_a_clearly_better_ratio() -> TestResult {
    assert_eq!(popularity(Some(1)), 1.0 + POPULARITY_CAP);
    assert_eq!(popularity(None), 1.0);
    assert_eq!(popularity(Some(51)), 1.0);
    let mut field = vec![
        candidate("popular", 50.0, [1.0, 4.0, 0.1], 256_000),
        candidate("better", 50.0, [0.95, 3.8, 0.095], 256_000),
    ];
    if let Some(first) = field.first_mut() {
        first.rank = Some(1);
    }
    let medium = picks("primary", &Mix::DEFAULT, &field)?
        .into_iter()
        .find(|(tag, ..)| tag == "medium")
        .map(|(_, id, _)| id);
    assert_eq!(medium.as_deref(), Some("better"));
    Ok(())
}

fn snapshot() -> Result<Value, Box<dyn Error>> {
    let list: Value = serde_json::from_str(OPENROUTER_LIST)?;
    let rows = list["data"].as_array().cloned().unwrap_or_default();
    let benchmarks: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            let aa = row.pointer("/benchmarks/artificial_analysis")?;
            Some(json!({
                "model_permaslug": row["canonical_slug"],
                "display_name": row["name"],
                "intelligence_index": aa["intelligence_index"],
                "coding_index": aa["coding_index"],
                "agentic_index": aa["agentic_index"],
                "pricing": {"prompt": row["pricing"]["prompt"], "completion": row["pricing"]["completion"]},
            }))
        })
        .collect();
    let tools: Vec<Value> = rows
        .iter()
        .filter(|row| {
            row["supported_parameters"]
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p == "tools"))
        })
        .map(|row| row["id"].clone())
        .collect();
    Ok(json!({
        "fetchedMs": 0,
        "benchmarks": {"data": benchmarks, "meta": {
            "as_of": "2026-09-05T00:00:00Z",
            "citation": "Artificial Analysis via OpenRouter",
        }},
        "rankings": {"data": [
            {"date": "2026-09-04", "model_permaslug": "anthropic/claude-opus-5-20260723", "total_tokens": 9},
            {"date": "2026-09-04", "model_permaslug": "other", "total_tokens": 99},
        ]},
        "tools": tools,
    }))
}

/// A catalog model joins its dated benchmark slug and its daily rank; one with no index and one
/// without a credential are counted out, never silently missing.
#[test]
fn the_snapshot_joins_the_catalog_and_counts_what_it_left_out() -> TestResult {
    let priced = |id: &str, input: f64| -> Result<yi_types::model::Model, Box<dyn Error>> {
        let mut entry = model(id, "openai-completions", "openrouter", "", None);
        entry.cost.input = serde_json::Number::from_f64(input).ok_or("price")?;
        entry.context_window = 1_000_000;
        Ok(entry)
    };
    let models = vec![
        priced("anthropic/claude-opus-5", 5.0)?,
        priced("acme/unbenched", 1.0)?,
    ];
    let snapshot = snapshot()?;
    let (scored, drops) = candidates(&snapshot, &models, &|_| true);
    assert_eq!((drops.catalog, drops.scored, drops.no_benchmark), (2, 1, 1));
    let first = scored.first().ok_or("nothing scored")?;
    assert_eq!(first.id, "openrouter/anthropic/claude-opus-5");
    assert_eq!(first.rank, Some(1));
    let (_, refused) = candidates(&snapshot, &models, &|_| false);
    assert_eq!((refused.scored, refused.no_credential), (0, 1));
    assert_eq!(
        citation(&snapshot),
        "data: Artificial Analysis via OpenRouter (CC BY 4.0), as of 2026-09-05T00:00:00Z"
    );
    Ok(())
}

fn ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A 429 names OpenRouter's limits and leaves the last snapshot readable; a refresh inside the
/// spacing window sends nothing.
#[test]
fn a_rate_limited_refresh_keeps_the_last_snapshot_and_names_the_limit() -> TestResult {
    let dir = Scratch::new("yi-recommend-429")?;
    let last = snapshot()?;
    std::fs::write(dir.join(SNAPSHOT), last.to_string())?;
    let limited =
        "HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    let (port, seen, _) = serve([limited])?;
    let base = format!("http://127.0.0.1:{port}");
    let error = refresh(&dir, &base, "k", "2026-09-04", MIN_REFRESH_MS, None)
        .err()
        .ok_or("a 429 refreshed")?;
    assert!(
        error.contains("HTTP 429") && error.contains("30 requests a minute"),
        "{error}"
    );
    assert_eq!(load(&dir)?, last);
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    let early = refresh(&dir, &base, "k", "2026-09-04", MIN_REFRESH_MS - 1, None)
        .err()
        .ok_or("refreshed inside the window")?;
    assert!(early.contains("waits 600s"), "{early}");
    Ok(())
}

/// The three answers become one snapshot whose rows the recommender reads back offline.
#[test]
fn a_refresh_writes_a_snapshot_the_offline_reader_scores() -> TestResult {
    let dir = Scratch::new("yi-recommend-refresh")?;
    let fake = snapshot()?;
    let list: Value = serde_json::from_str(OPENROUTER_LIST)?;
    let (port, _, requests) = serve([ok(&fake["benchmarks"]), ok(&fake["rankings"]), ok(&list)])?;
    let count = refresh(
        &dir,
        &format!("http://127.0.0.1:{port}"),
        "k",
        "2026-09-04",
        5,
        None,
    )?;
    assert_eq!(count, 4);
    let requests = requests.join().map_err(|_| "server thread")?;
    assert!(
        requests
            .iter()
            .all(|r| r.to_lowercase().contains("authorization: bearer k"))
    );
    assert!(
        requests
            .first()
            .is_some_and(|r| r.contains("source=artificial-analysis"))
    );
    let stored = load(&dir)?;
    assert_eq!(stored["benchmarks"], fake["benchmarks"]);
    assert_eq!(stored["tools"], fake["tools"]);
    assert_eq!(stored["fetchedMs"], 5);
    Ok(())
}

#[test]
fn a_missing_snapshot_names_the_refresh_command() -> TestResult {
    let dir = Scratch::new("yi-recommend-none")?;
    let error = load(&dir).err().ok_or("loaded nothing")?;
    assert!(error.contains("yi models refresh"), "{error}");
    Ok(())
}
