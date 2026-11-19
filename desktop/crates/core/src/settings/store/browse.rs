//! Looking through the settings: the groups with their counts, and a search.

use crate::settings::schema::Setting;

/// One group of the tree on the left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub total: usize,
    /// How many of its settings differ from their defaults.
    pub changed: usize,
}

/// The groups in the order their first setting appears, each with its counts.
pub fn groups(rows: &[Setting]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for row in rows {
        let at = match out.iter().position(|group| group.name == row.group) {
            Some(at) => at,
            None => {
                out.push(Group {
                    name: row.group.to_string(),
                    total: 0,
                    changed: 0,
                });
                out.len().saturating_sub(1)
            }
        };
        if let Some(group) = out.get_mut(at) {
            group.total = group.total.saturating_add(1);
            group.changed = group.changed.saturating_add(usize::from(row.changed()));
        }
    }
    out
}

pub fn changed(rows: &[Setting]) -> usize {
    rows.iter().filter(|row| row.changed()).count()
}

/// The settings whose name, sentence, key or group holds every word of `query`, in any case.
pub fn search<'a>(rows: &'a [Setting], query: &str) -> Vec<&'a Setting> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter()
        .filter(|row| {
            let hay = format!(
                "{} {} {} {}",
                row.name,
                row.description,
                row.key.as_str(),
                row.group
            )
            .to_lowercase();
            words.iter().all(|word| hay.contains(word.as_str()))
        })
        .collect()
}
