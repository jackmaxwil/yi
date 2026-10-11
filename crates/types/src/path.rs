//! An archive-entry path check for unpacks of paths the code did not write: a part that could
//! leave the archive or the staging directory refuses it. Siblings reject non-normal components.

/// The first part of a relative path that could escape the directory it lands in:
/// an empty part, `.` or `..`. `None` for a path that stays inside.
pub fn escaping_part(relative: &str) -> Option<&str> {
    relative
        .split('/')
        .find(|part| part.is_empty() || *part == "." || *part == "..")
}

#[cfg(test)]
mod tests {
    use super::escaping_part;

    #[test]
    fn the_escaping_parts_are_empty_dot_and_dotdot() {
        assert_eq!(escaping_part(""), Some(""));
        assert_eq!(escaping_part("a/./b"), Some("."));
        assert_eq!(escaping_part("a/../b"), Some(".."));
        assert_eq!(escaping_part("a//b"), Some(""));
        assert_eq!(escaping_part("skills/ripwire-orient/map.md"), None);
        assert_eq!(escaping_part("./skills/x.md"), Some("."));
    }
}
