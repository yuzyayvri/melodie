use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as unix seconds. Used for `added_at`, `first_seen`,
/// `decided_at`, `synced_at`, `next_attempt_at` columns — plain integers,
/// no chrono dependency for a handful of timestamp columns.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
