//! The state of a file, decided in one place.
//!
//! A file is in exactly one of seven states: ready (`clean`), embedding open, warning, blocker, not
//! verified, inspection failed or missing. The state is derived from the discovery state, the stored
//! check verdict and whether a subtitle file still waits to be embedded; the stored verdict stays
//! `clean` for a file with an open embedding. Filters, sorting, the column view, the overview and the
//! candidate test for Prepare all ask this module, in SQL (`state_sql`, `rank_sql`) or in Rust
//! (`state_of`, `rank`), and the two answer alike.

/// Every state token in the order of the status sort: blocker, inspection failed, missing, warning,
/// embedding open, not verified, ready.
pub const RANKED: [&str; 7] = [
    "blocker",
    "probe_failed",
    "missing",
    "warning",
    "embed_open",
    "unknown",
    "clean",
];

/// The states in the order the filter offers them.
pub const STATES: [&str; 7] = [
    "clean",
    "embed_open",
    "warning",
    "blocker",
    "unknown",
    "probe_failed",
    "missing",
];

/// The properties of a file that hold whatever its state is and may overlap.
pub const TRAITS: [&str; 5] = [
    "convert",
    "encode_audio",
    "encode_video",
    "embed",
    "converted",
];

/// The states in which Prepare has something to do with a present file.
pub const PREPARABLE: [&str; 3] = ["warning", "blocker", "embed_open"];

/// The rank of a token that is no state: it sorts after every state.
pub const UNRANKED: u8 = 9;

/// The rank of a state token in the status sort; a token that is no state sorts last.
#[must_use]
pub fn rank(token: &str) -> u8 {
    RANKED
        .iter()
        .position(|known| *known == token)
        .map_or(UNRANKED, |index| u8::try_from(index).unwrap_or(UNRANKED))
}

/// The state token of a file. A stored verdict this module does not know is its own token.
#[must_use]
pub fn state_of<'a>(discovery: &str, compliance: &'a str, embed_open: bool) -> &'a str {
    if discovery == "missing" {
        "missing"
    } else if compliance == "clean" && embed_open {
        "embed_open"
    } else {
        compliance
    }
}

/// Prepare would take the file: it is present and has a verdict to repair, a wish to be fixed or a
/// subtitle file to embed.
#[must_use]
pub fn is_preparable(
    discovery: &str,
    compliance: &str,
    desired_action: &str,
    embed_open: bool,
) -> bool {
    discovery == "present"
        && (PREPARABLE.contains(&state_of(discovery, compliance, embed_open))
            || desired_action == "fix"
            || embed_open)
}

/// The SQL expression (table alias `m`) that gives a row's state token, given a bound JSON list of
/// the ids of the files with an open embedding (`param` is `?` or `?N`).
#[must_use]
pub fn state_sql(param: &str) -> String {
    format!(
        "(CASE WHEN m.discovery_state='missing' THEN 'missing' \
         WHEN m.compliance_state='clean' AND {} THEN 'embed_open' \
         ELSE m.compliance_state END)",
        crate::media_sql::embed_open(param)
    )
}

/// The SQL expression of a row's rank in the status sort, as `rank` gives it. It binds the list of
/// open ids once.
#[must_use]
pub fn rank_sql(param: &str) -> String {
    let arms: Vec<String> = RANKED
        .iter()
        .enumerate()
        .map(|(index, token)| format!("WHEN '{token}' THEN {index}"))
        .collect();
    format!(
        "(CASE {} {} ELSE {UNRANKED} END)",
        state_sql(param),
        arms.join(" ")
    )
}

/// The SQL test (table alias `m`) that Prepare would take the file, as `is_preparable` decides it
/// for a present file: a preparable state, the wish to be fixed or an open embedding.
///
/// The JSON list of the open ids is used twice, so it is a numbered placeholder: `open_ids` is the
/// `N` of `?N`. A bare `?` would bind it once per use and shift every parameter after it.
#[must_use]
pub fn preparable_sql(open_ids: u8) -> String {
    let open_param = format!("?{open_ids}");
    let states: Vec<String> = PREPARABLE.iter().map(|s| format!("'{s}'")).collect();
    format!(
        "({} IN ({}) OR m.desired_action='fix' OR {})",
        state_sql(&open_param),
        states.join(","),
        crate::media_sql::embed_open(&open_param)
    )
}

/// The only thing a run would do to the file is rewrite its chunks (ADR 0010).
///
/// True when the stored Änderungsart is a remux, every stored finding is the rule `chunk-length`
/// and nothing else asks for Prepare: no wish to be fixed and no open embedding. A schedule does
/// not take such a file, so no night rewrites the whole library; a manual job still does.
#[must_use]
pub fn only_chunk_length_sql(open_ids: u8) -> String {
    format!(
        "(m.planned_change='remux' AND json_valid(m.compliance_reasons) \
         AND EXISTS (SELECT 1 FROM json_each(m.compliance_reasons)) \
         AND NOT EXISTS (SELECT 1 FROM json_each(m.compliance_reasons) \
                         WHERE value NOT LIKE '[%] chunk-length: %') \
         AND m.desired_action<>'fix' AND NOT {})",
        crate::media_sql::embed_open(&format!("?{open_ids}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// (discovery, verdict, embedding open, token, rank)
    const TABLE: [(&str, &str, bool, &str, u8); 12] = [
        ("present", "clean", false, "clean", 6),
        ("present", "clean", true, "embed_open", 4),
        ("present", "warning", false, "warning", 3),
        ("present", "warning", true, "warning", 3),
        ("present", "blocker", false, "blocker", 0),
        ("present", "blocker", true, "blocker", 0),
        ("present", "unknown", false, "unknown", 5),
        ("present", "unknown", true, "unknown", 5),
        ("present", "probe_failed", false, "probe_failed", 1),
        ("missing", "clean", false, "missing", 2),
        ("missing", "blocker", false, "missing", 2),
        ("missing", "clean", true, "missing", 2),
    ];

    #[test]
    fn rust_decides_every_combination() {
        for (discovery, verdict, open, token, rank_expected) in TABLE {
            let state = state_of(discovery, verdict, open);
            assert_eq!(state, token, "{discovery} {verdict} {open}");
            assert_eq!(rank(state), rank_expected, "{token}");
        }
    }

    #[test]
    fn sql_decides_like_rust() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE media_files(id INTEGER PRIMARY KEY, discovery_state TEXT, compliance_state TEXT, desired_action TEXT)",
        )
        .unwrap();
        let mut open_ids = Vec::new();
        for (index, (discovery, verdict, open, _, _)) in TABLE.iter().enumerate() {
            let id = i64::try_from(index).unwrap() + 1;
            conn.execute(
                "INSERT INTO media_files VALUES (?1,?2,?3,'none')",
                rusqlite::params![id, discovery, verdict],
            )
            .unwrap();
            if *open {
                open_ids.push(id);
            }
        }
        let open_json = serde_json::to_string(&open_ids).unwrap();
        let mut statement = conn
            .prepare(&format!(
                "SELECT id, {}, {} FROM media_files m ORDER BY id",
                state_sql("?1"),
                rank_sql("?1")
            ))
            .unwrap();
        let rows: Vec<(i64, String, u8)> = statement
            .query_map([&open_json], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), TABLE.len());
        for (row, (_, _, _, token, rank_expected)) in rows.iter().zip(TABLE) {
            assert_eq!(row.1, token, "id {}", row.0);
            assert_eq!(row.2, rank_expected, "id {}", row.0);
        }
    }

    #[test]
    fn the_lists_cover_the_same_states() {
        let mut ranked = RANKED.to_vec();
        let mut listed = STATES.to_vec();
        ranked.sort_unstable();
        listed.sort_unstable();
        assert_eq!(ranked, listed);
    }

    #[test]
    fn preparable_follows_state_wish_and_embedding() {
        assert!(is_preparable("present", "warning", "none", false));
        assert!(is_preparable("present", "blocker", "none", false));
        assert!(is_preparable("present", "clean", "none", true));
        assert!(is_preparable("present", "unknown", "fix", false));
        assert!(!is_preparable("present", "clean", "none", false));
        assert!(!is_preparable("present", "unknown", "none", false));
        assert!(!is_preparable("missing", "blocker", "fix", true));
    }

    /// The tokens the frontend lists must match the server's, in the same order.
    #[test]
    fn frontend_filter_lists_match() {
        let source = include_str!("../../../../frontend/src/features/files/filters.ts");
        let block = |name: &str| -> Vec<String> {
            let start = source.find(&format!("export const {name}")).unwrap();
            let rest = &source[start..];
            let end = rest.find("];").unwrap();
            rest[..end]
                .split("token: \"")
                .skip(1)
                .map(|part| part.split('"').next().unwrap().to_owned())
                .collect()
        };
        assert_eq!(block("STATE_FILTERS"), STATES);
        assert_eq!(block("TRAIT_FILTERS"), TRAITS);
    }
}
