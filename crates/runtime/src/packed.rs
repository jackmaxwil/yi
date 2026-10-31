//! Incident: 54 KiB of raw prompt text cost the dist binary four 16 KiB `__TEXT` pages of the
//! measured size ratchet, so `build.rs` deflates every prompt and the binary carries the deflate.

/// Invariant: `build.rs` proves the round trip before the crate compiles, so a decode that
/// failed here would name itself in the prompt rather than reach the model as silence.
pub(crate) fn inflate(bytes: &'static [u8]) -> String {
    miniz_oxide::inflate::decompress_to_vec_zlib(bytes)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_else(|error| format!("[prompt unavailable: {error:?}]"))
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
    use crate::{auto_review, ext::install, goal, subagent::reader};
    use std::sync::LazyLock;

    /// A static handed another stem ships that prompt to the model; the build cannot see it.
    #[test]
    fn every_packed_prompt_reads_as_its_own_file() {
        let rows: [(&str, &LazyLock<String>, &str); 10] = [
            (
                "auto_review.md",
                &auto_review::AUTO_REVIEW,
                include_str!("prompts/auto_review.md"),
            ),
            (
                "har-core.md",
                &install::HAR_CORE,
                include_str!("prompts/har-core.md"),
            ),
            (
                "orchestrate.md",
                &install::ORCHESTRATE,
                include_str!("prompts/orchestrate.md"),
            ),
            (
                "reader.md",
                &reader::READER,
                include_str!("prompts/reader.md"),
            ),
            (
                "reader_fenced.md",
                &reader::READER_FENCED,
                include_str!("prompts/reader_fenced.md"),
            ),
            (
                "reader_unfenced.md",
                &reader::READER_UNFENCED,
                include_str!("prompts/reader_unfenced.md"),
            ),
            (
                "worker.md",
                &reader::WORKER,
                include_str!("prompts/worker.md"),
            ),
            (
                "continuation.md",
                &goal::CONTINUATION,
                include_str!("goal/prompts/continuation.md"),
            ),
            (
                "budget_limit.md",
                &goal::BUDGET_LIMIT,
                include_str!("goal/prompts/budget_limit.md"),
            ),
            (
                "objective_updated.md",
                &goal::OBJECTIVE_UPDATED,
                include_str!("goal/prompts/objective_updated.md"),
            ),
        ];
        let wrong: Vec<&str> = rows
            .iter()
            .filter(|(_, packed, source)| packed.as_str() != *source)
            .map(|(name, _, _)| *name)
            .collect();
        assert!(wrong.is_empty(), "read as another file: {wrong:?}");
    }

    #[test]
    fn the_shipped_graph_is_its_own_file() -> Result<(), serde_json::Error> {
        let source: yi_types::graph::Graph =
            serde_json::from_str(include_str!("prompts/graph.json"))?;
        assert_eq!(crate::affordance::shipped(), &source);
        Ok(())
    }
}
