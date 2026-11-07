//! SQL fragments over `media_files` that several queries share (lists, counters, schedule hits).
//!
//! Every fragment expects the table alias `m`, and the subquery alias `sibling` stays inside.

/// The container suffixes that need no conversion, as an SQL list.
macro_rules! mp4_like_list {
    () => {
        "('.mp4','.m4v','.mov')"
    };
}

/// A sibling `.mp4` of the same name exists in the same library.
macro_rules! sibling_mp4_exists {
    () => {
        "EXISTS (SELECT 1 FROM media_files sibling WHERE sibling.root_id=m.root_id \
         AND sibling.discovery_state='present' \
         AND sibling.relative_path=substr(m.relative_path,1,length(m.relative_path)-length(m.suffix))||'.mp4')"
    };
}

/// The file already has an MP4-like suffix.
pub const MP4_LIKE: &str = concat!("lower(m.suffix) IN ", mp4_like_list!());

/// The file has a suffix that has to be converted to MP4 first.
pub const NOT_MP4_LIKE: &str = concat!("lower(m.suffix) NOT IN ", mp4_like_list!());

/// The converted original is kept: a non-MP4 file whose `.mp4` sibling exists.
pub const CONVERTED: &str = concat!(
    "(lower(m.suffix) NOT IN ",
    mp4_like_list!(),
    " AND ",
    sibling_mp4_exists!(),
    ")"
);

/// The file still has to be converted: a non-MP4 file without a `.mp4` sibling.
pub const TO_CONVERT: &str = concat!(
    "(lower(m.suffix) NOT IN ",
    mp4_like_list!(),
    " AND NOT ",
    sibling_mp4_exists!(),
    ")"
);

/// The check found that the video must be re-encoded.
pub const NEEDS_REENCODE: &str = "m.needs_reencode=1";

/// The file's embedding is open, given a bound JSON list of the open ids (`param` is `?` or `?N`).
#[must_use]
pub fn embed_open(param: &str) -> String {
    format!("m.id IN (SELECT value FROM json_each({param}))")
}

/// The file's embedding is not open, given a bound JSON list of the open ids.
#[must_use]
pub fn embed_not_open(param: &str) -> String {
    format!("m.id NOT IN (SELECT value FROM json_each({param}))")
}

/// What makes a file a candidate for a repair or conversion schedule.
///
/// Excludes converted originals; takes MP4-likes only when they have something to repair or
/// embed; takes re-encode files only when `allow_reencode_param` is true.
#[must_use]
pub fn change_candidate(allow_reencode_param: &str, open_ids_param: &str) -> String {
    format!(
        "NOT {CONVERTED} AND ({NOT_MP4_LIKE} OR m.compliance_state IN ('warning','blocker') \
         OR m.desired_action='fix' OR {open}) AND ({allow_reencode_param} OR m.needs_reencode=0)",
        open = embed_open(open_ids_param)
    )
}
