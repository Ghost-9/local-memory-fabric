use crate::db::busy_message;
use crate::exit_code_for;
use crate::paths::safe_relpath;
use crate::policy::{apple_path_re, secret_signature};
use crate::text::tokenize;
use serde_json::json;

#[test]
fn busy_messages_are_told_apart_from_broken_indexes() {
    // Retry is right for the first, pointless for the second; both must be greppable.
    assert!(busy_message("database is locked", "dispatch").contains("busy"));
    assert!(busy_message("database is locked", "dispatch").contains("retry shortly"));
    assert!(busy_message("database is busy", "dispatch").contains("busy"));
    // A genuinely broken index must NOT be dressed up as transient contention.
    let broken = busy_message("database disk image is malformed", "dispatch");
    assert!(!broken.contains("busy"), "malformed index misreported as busy");
    assert_eq!(broken, "database disk image is malformed");
}

#[test]
fn exit_codes_separate_a_result_from_no_result_and_from_an_error() {
    // A real result is 0, including for operations that carry no `results` array.
    assert_eq!(
        exit_code_for(&json!({"ok": true, "results": [{"source_ref": "projects/a.md"}]})),
        0
    );
    assert_eq!(exit_code_for(&json!({"ok": true, "documents": 114})), 0);
    // Nothing to return is 1, not an error, and not a success.
    assert_eq!(
        exit_code_for(&json!({"ok": true, "results": [], "abstention": "no indexed source contains these terms"})),
        1
    );
    assert_eq!(
        exit_code_for(&json!({"ok": true, "results": [], "shown": 0, "total_matches": 0, "abstention": null})),
        1
    );
    // A policy refusal is its own outcome, not a failure of the request.
    assert_eq!(
        exit_code_for(&json!({"ok": false, "blocked": "Proposal contents are available only to an explicitly allowed local model provider."})),
        2
    );
    // A real error is 3.
    assert_eq!(exit_code_for(&json!({"ok": false, "error": "invalid project identifier"})), 3);
}

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
