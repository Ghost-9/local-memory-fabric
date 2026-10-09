use crate::paths::safe_relpath;
use crate::policy::{apple_path_re, secret_signature};
use crate::text::tokenize;

#[test]
fn safe_paths_reject_traversal() {
    assert!(safe_relpath("../secret.md").is_err());
    assert_eq!(safe_relpath("projects/a.md").unwrap(), "projects/a.md");
}
#[test]
fn tokenization_is_casefolded_and_deduplicated() {
    assert_eq!(
        tokenize("SQLite sqlite FTS5 the")
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["fts5", "sqlite"]
    );
}
#[test]
fn secret_filter_catches_underscore_service_role_key() {
    let candidate = format!(
        "SUPABASE_SERVICE_ROLE_KEY={}",
        "CANDIDATE".to_string() + &"X".repeat(24)
    );
    assert_eq!(secret_signature(&candidate), Some("credential_assignment"));
}
#[test]
fn secret_filter_ignores_placeholder() {
    assert_eq!(
        secret_signature("SUPABASE_SERVICE_ROLE_KEY=YOUR_API_KEY_HERE"),
        None
    );
}
#[test]
fn apple_paths_are_rejected() {
    assert!(apple_path_re().is_match("Apple Notes Export"));
}
