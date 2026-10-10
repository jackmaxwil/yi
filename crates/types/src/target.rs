//! The one table matcher for pinned builds, shared by uv's install and the plugin table.

/// The refusal every pinned-build surface names its target with.
pub fn refusal(label: &str) -> String {
    format!(
        "yi pins no {label} build for {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// The entry for this build's target in a table keyed by os and arch, or [`refusal`].
pub fn for_target<T: Copy>(
    label: &str,
    mut entries: impl Iterator<Item = (&'static str, &'static str, T)>,
) -> Result<T, String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    entries
        .find(|(o, a, _)| *o == os && *a == arch)
        .map(|(_, _, entry)| entry)
        .ok_or_else(|| refusal(label))
}

#[cfg(test)]
mod tests {
    type TestResult = Result<(), String>;
    use super::{for_target, refusal};

    #[test]
    fn a_table_with_this_target_serves_it_and_one_without_gets_the_refusal() -> TestResult {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        let table = [(os, arch, "entry")];
        assert_eq!(for_target("tool", table.iter().copied())?, "entry");
        let elsewhere = [("plan9", "vax", "entry")];
        let error = for_target("tool", elsewhere.iter().copied())
            .err()
            .ok_or("refused no target")?;
        assert_eq!(error, refusal("tool"));
        assert!(error.contains(&format!("{os}/{arch}")), "{error}");
        Ok(())
    }
}
