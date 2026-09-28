use std::collections::{BTreeMap, BTreeSet};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const STOP: [&str; 61] = [
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "is", "are", "be", "it", "this",
    "that", "with", "as", "at", "by", "from", "not", "no", "do", "does", "did", "i", "you", "we",
    "they", "my", "your", "our", "its", "was", "were", "will", "would", "can", "could", "should",
    "have", "has", "had", "if", "then", "so", "but", "just", "what", "which", "when", "how", "why",
    "there", "here", "all", "any", "some", "more", "most",
];

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub fn tokens(text: &str) -> Vec<String> {
    let lower: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < lower.len() {
        if !lower.get(at).copied().is_some_and(word) {
            at = at.saturating_add(1);
            continue;
        }
        let start = at;
        loop {
            while lower.get(at).copied().is_some_and(word) {
                at = at.saturating_add(1);
            }
            let joined = lower.get(at).is_some_and(|c| matches!(c, '.' | '/' | '-'))
                && lower.get(at.saturating_add(1)).copied().is_some_and(word);
            if !joined {
                break;
            }
            at = at.saturating_add(1);
        }
        let token: String = lower.get(start..at).unwrap_or_default().iter().collect();
        if !STOP.contains(&token.as_str()) {
            out.push(token);
        }
    }
    out
}

struct Doc {
    terms: BTreeMap<String, u32>,
    len: f64,
}

pub struct Bm25 {
    docs: Vec<Doc>,
    df: BTreeMap<String, u32>,
    avgdl: f64,
}

impl Bm25 {
    pub fn new<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let docs: Vec<Doc> = texts
            .into_iter()
            .map(|text| {
                let mut terms: BTreeMap<String, u32> = BTreeMap::new();
                let all = tokens(text);
                for token in &all {
                    let slot = terms.entry(token.clone()).or_insert(0);
                    *slot = slot.saturating_add(1);
                }
                Doc {
                    terms,
                    len: all.len() as f64,
                }
            })
            .collect();
        let mut df: BTreeMap<String, u32> = BTreeMap::new();
        for doc in &docs {
            for term in doc.terms.keys() {
                let slot = df.entry(term.clone()).or_insert(0);
                *slot = slot.saturating_add(1);
            }
        }
        let total: f64 = docs.iter().map(|doc| doc.len).sum();
        let avgdl = if docs.is_empty() {
            0.0
        } else {
            total / docs.len() as f64
        };
        Self { docs, df, avgdl }
    }

    pub fn rank(&self, query: &str) -> Vec<(usize, f64)> {
        let n = self.docs.len() as f64;
        let terms: BTreeSet<String> = tokens(query).into_iter().collect();
        let mut scored: Vec<(usize, f64)> = self
            .docs
            .iter()
            .enumerate()
            .map(|(at, doc)| {
                let norm = K1 * (1.0 - B + B * doc.len / self.avgdl.max(f64::MIN_POSITIVE));
                let score = terms
                    .iter()
                    .filter_map(|term| {
                        let tf = f64::from(*doc.terms.get(term)?);
                        let seen = f64::from(*self.df.get(term)?);
                        let idf = (1.0 + (n - seen + 0.5) / (seen + 0.5)).ln();
                        Some(idf * tf / (tf + norm))
                    })
                    .sum::<f64>();
                (at, score)
            })
            .filter(|(_, score)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_keep_paths_and_identifiers_whole_and_drop_function_words() {
        assert_eq!(
            tokens("Never run `git reset --hard` in ~/.yi/daemon.sock or scripts/forge_pr.py — É!"),
            vec![
                "never",
                "run",
                "git",
                "reset",
                "hard",
                "yi/daemon.sock",
                "scripts/forge_pr.py"
            ]
        );
    }

    #[test]
    fn the_note_that_shares_the_rare_word_ranks_first_and_nothing_matches_nothing() {
        let bm = Bm25::new(["main main main", "sync zebra quux", "main orb lane"]);
        let ranked = bm.rank("sync main");
        assert_eq!(ranked.first().map(|hit| hit.0), Some(1), "{ranked:?}");
        assert!(bm.rank("   ").is_empty());
        assert!(bm.rank("absent").is_empty());
        assert!(Bm25::new([]).rank("anything").is_empty());
    }
}
