//! Helpers and bounds shared by the filters of the file list and the job list.

/// Longest accepted value of a filter parameter, in characters.
pub(crate) const MAX_PARAM: usize = 128;

/// Longest accepted list of opaque library ids (`root_id`), in characters: several libraries make
/// a long list of ids.
pub(crate) const MAX_ID_LIST: usize = 1024;

/// The non-empty entries of a comma-separated list.
pub(crate) fn tokens(value: Option<&str>) -> Vec<&str> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|token| !token.is_empty())
        .collect()
}

/// `count` SQL placeholders, comma-separated, for an `IN (…)` list.
pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

#[cfg(test)]
mod tests {
    use super::{placeholders, tokens};

    #[test]
    fn tokens_skip_empty_entries() {
        assert_eq!(tokens(Some("a,,b,")), vec!["a", "b"]);
        assert!(tokens(None).is_empty());
    }

    #[test]
    fn placeholders_match_the_count() {
        assert_eq!(placeholders(3), "?,?,?");
    }
}
