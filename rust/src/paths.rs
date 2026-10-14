use chrono::{Datelike, NaiveDate, Timelike, Utc};
use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Component, Path, PathBuf};

pub(crate) const VERSION: &str = "2.0.0";

/// C1/C4 - the index format version, separate from the engine version.
///
/// These move independently: the engine can fix a bug without touching the format (no migration
/// needed), and the format can change without a new engine release being what forces it. An
/// expand-only ladder means a bump is safe to roll back by running the older binary.
pub(crate) const SCHEMA_VERSION: &str = "3";
pub(crate) const MAX_REQUEST_BYTES: usize = 131_072;
pub(crate) const MAX_RESULT_CHARS_DEFAULT: usize = 6_000;
/// Ceiling for one context packet. 6_000 tokens x 4 chars - the previous Python clamp, now
/// applied to both engines so an omitted budget cannot mean unbounded.
pub(crate) const MAX_CONTEXT_CHARS_DEFAULT: usize = 24_000;

/// UTC timestamp, byte-identical to the Python engine's `utc_now()`.
///
/// Python emits `isoformat(timespec="seconds")`, which renders the offset as `+00:00`;
/// Rust's `to_rfc3339_opts` renders `Z`. Both are valid ISO 8601, but the two engines must
/// agree byte-for-byte or every stored timestamp is a parity mismatch waiting to be found.
/// Formatted manually rather than via `format!` gymnastics on a chrono type.
pub(crate) fn now_utc() -> String {
    let now = Utc::now();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

pub(crate) fn safe_relpath(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err("source reference is not an allowed relative path".into());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            _ => return Err("source reference is not an allowed relative path".into()),
        }
    }
    if parts.is_empty() {
        return Err("source reference is not an allowed relative path".into());
    }
    Ok(parts.join("/"))
}

pub(crate) fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.file_type().is_symlink() {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

pub(crate) fn set_open_mode(options: &mut OpenOptions, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    {
        let _ = (options, mode);
    }
}

pub(crate) fn parse_optional_date(value: Option<&str>) -> Result<Option<NaiveDate>, ()> {
    match value {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| ()),
    }
}
