//! Incident: 54 KiB of raw prompt text cost the dist binary four 16 KiB `__TEXT` pages of the
//! measured size ratchet, so `build.rs` deflates every prompt and the binary carries the deflate.

#[cfg(test)]
use std::{collections::BTreeSet, sync::Mutex};

#[cfg(test)]
static INFLATED: Mutex<BTreeSet<&'static [u8]>> = Mutex::new(BTreeSet::new());

pub(crate) fn inflate(bytes: &'static [u8]) -> String {
    #[cfg(test)]
    INFLATED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(bytes);
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(bytes).unwrap_or_default();
    String::from_utf8(raw).unwrap_or_default()
}

/// The prompt `build.rs` deflated from the source file stemmed `$stem`, so a static's name and
/// the bytes it is handed are spelled once.
macro_rules! packed {
    ($stem:literal) => {
        std::sync::LazyLock::new(|| {
            $crate::packed::inflate(include_bytes!(concat!(env!("OUT_DIR"), "/", $stem, ".zz")))
        })
    };
}
pub(crate) use packed;

#[cfg(test)]
mod tests {
    use super::INFLATED;
    use crate::{auto_review, ext::install, goal, subagent::reader};
    use std::sync::LazyLock;
    /// Each blob must reach its accessor through [`crate::packed::inflate`]; wrong stems fail too.
    #[test]
    fn every_packed_prompt_reads_as_its_own_file() {
        let rows: [(&str, &LazyLock<String>, &[u8], &str); 14] = [
            (
                "identity",
                &crate::IDENTITY,
                include_bytes!(concat!(env!("OUT_DIR"), "/identity.zz")),
                include_str!("prompts/identity.md"),
            ),
            (
                "doctrine",
                &crate::DOCTRINE,
                include_bytes!(concat!(env!("OUT_DIR"), "/doctrine.zz")),
                include_str!("prompts/doctrine.md"),
            ),
            (
                "delegation",
                &crate::DELEGATION,
                include_bytes!(concat!(env!("OUT_DIR"), "/delegation.zz")),
                include_str!("prompts/delegation.md"),
            ),
            (
                "unattended",
                &crate::UNATTENDED,
                include_bytes!(concat!(env!("OUT_DIR"), "/unattended.zz")),
                include_str!("prompts/unattended.md"),
            ),
            (
                "auto_review",
                &auto_review::AUTO_REVIEW,
                include_bytes!(concat!(env!("OUT_DIR"), "/auto_review.zz")),
                include_str!("prompts/auto_review.md"),
            ),
            (
                "har-core",
                &install::HAR_CORE,
                include_bytes!(concat!(env!("OUT_DIR"), "/har-core.zz")),
                include_str!("prompts/har-core.md"),
            ),
            (
                "orchestrate",
                &install::ORCHESTRATE,
                include_bytes!(concat!(env!("OUT_DIR"), "/orchestrate.zz")),
                include_str!("prompts/orchestrate.md"),
            ),
            (
                "reader",
                &reader::READER,
                include_bytes!(concat!(env!("OUT_DIR"), "/reader.zz")),
                include_str!("prompts/reader.md"),
            ),
            (
                "reader_fenced",
                &reader::READER_FENCED,
                include_bytes!(concat!(env!("OUT_DIR"), "/reader_fenced.zz")),
                include_str!("prompts/reader_fenced.md"),
            ),
            (
                "reader_unfenced",
                &reader::READER_UNFENCED,
                include_bytes!(concat!(env!("OUT_DIR"), "/reader_unfenced.zz")),
                include_str!("prompts/reader_unfenced.md"),
            ),
            (
                "worker",
                &reader::WORKER,
                include_bytes!(concat!(env!("OUT_DIR"), "/worker.zz")),
                include_str!("prompts/worker.md"),
            ),
            (
                "continuation",
                &goal::CONTINUATION,
                include_bytes!(concat!(env!("OUT_DIR"), "/continuation.zz")),
                include_str!("goal/prompts/continuation.md"),
            ),
            (
                "budget_limit",
                &goal::BUDGET_LIMIT,
                include_bytes!(concat!(env!("OUT_DIR"), "/budget_limit.zz")),
                include_str!("goal/prompts/budget_limit.md"),
            ),
            (
                "objective_updated",
                &goal::OBJECTIVE_UPDATED,
                include_bytes!(concat!(env!("OUT_DIR"), "/objective_updated.zz")),
                include_str!("goal/prompts/objective_updated.md"),
            ),
        ];
        for (_, live, _, _) in &rows {
            let _ = live.as_str();
        }
        let inflated = INFLATED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let wrong: Vec<&str> = rows
            .iter()
            .filter(|(_, live, packed, source)| {
                !inflated.contains(packed)
                    || packed.len() >= source.len()
                    || packed.first() != Some(&0x78)
                    || miniz_oxide::inflate::decompress_to_vec_zlib(packed)
                        .ok()
                        .as_deref()
                        != Some(source.as_bytes())
                    || live.as_str() != *source
            })
            .map(|(name, _, _, _)| *name)
            .collect();
        assert!(
            wrong.is_empty(),
            "not deflated from its own file: {wrong:?}"
        );
    }

    #[test]
    fn the_shipped_graph_reads_as_its_own_file() -> Result<(), serde_json::Error> {
        let source = include_str!("prompts/graph.json");
        let packed = include_bytes!(concat!(env!("OUT_DIR"), "/graph.zz"));
        assert!(packed.len() < source.len() && packed.first() == Some(&0x78));
        let graph: yi_types::graph::Graph = serde_json::from_str(source)?;
        assert_eq!(crate::affordance::shipped(), &graph);
        assert!(
            INFLATED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&packed.as_slice()),
            "shipped graph did not pass through inflate"
        );
        Ok(())
    }
}
