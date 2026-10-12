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
    assert!(broken.contains("corrupt"));
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
    // Verified against the Python engine: tokens("SQLite sqlite FTS5 the") == ["fts5","sqlite"].
    // `SQLite` splits to [sq, lite] and `FTS5` to [fts, 5]; after dropping sub-3-char parts only
    // ONE part remains in each case, so neither is compound and no partial token is indexed.
    // That filter-then-test ordering is what keeps the engines identical - see
    // `camel_split_matches_the_python_engine` for the cases that DO split.
    let got: Vec<String> = tokenize("SQLite sqlite FTS5 the").into_iter().collect();
    assert_eq!(got, vec!["fts5", "sqlite"]);
    assert!(got.contains(&"sqlite".to_string()), "original token must survive splitting");
    assert!(got.contains(&"fts5".to_string()), "original token must survive splitting");
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


#[test]
fn camel_split_matches_the_python_engine() {
    use crate::text::split_identifier;
    // These are the exact expectations from the Python engine's CAMEL_SPLIT_RE, minus parts
    // shorter than 3 characters (which the caller filters).
    assert_eq!(split_identifier("CliffCompaction"), vec!["cliff", "compaction"]);
    assert_eq!(split_identifier("parseNotes"), vec!["parse", "notes"]);
    assert_eq!(split_identifier("localMemoryFabric"), vec!["local", "memory", "fabric"]);
    assert_eq!(split_identifier("BM25Cache"), vec!["bm", "25", "cache"]);
    assert_eq!(split_identifier("FTS5-Lite"), vec!["fts", "5", "lite"]);
    // An acronym with no case boundary stays whole - the reason BM25 must still match BM25.
    assert_eq!(split_identifier("KV"), vec!["kv"]);
}


#[test]
fn proposals_lifecycle_show_and_list() {
    let tmp = std::env::temp_dir().join(format!("mf-test-{}", uuid::Uuid::new_v4().simple()));
    let home = tmp.join("home");
    let vault = tmp.join("vault");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(vault.join("projects")).unwrap();
    std::fs::write(vault.join("projects/demo.md"), "# Project Demo\n\n## Overview\nInitial content.\n").unwrap();
    let fabric = crate::MemoryFabric::with_paths(home.clone(), vault.clone()).unwrap();
    fabric.index(true).unwrap();

    let list_empty = fabric.proposal_list().unwrap();
    assert_eq!(list_empty["ok"], true);
    assert_eq!(list_empty["proposals"].as_array().unwrap().len(), 0);

    let prop = fabric.propose(&json!({
        "project": "demo",
        "target": "projects/demo.md",
        "section": "Overview",
        "text": "Proposed addition.",
        "provider": "ollama"
    })).unwrap();
    assert_eq!(prop["ok"], true);
    let id = prop["proposal_id"].as_str().unwrap();

    let shown = fabric.proposal_show(id, Some("ollama")).unwrap();
    assert_eq!(shown["ok"], true);
    assert_eq!(shown["status"], "proposed_review_required");

    let list = fabric.proposal_list().unwrap();
    assert_eq!(list["proposals"].as_array().unwrap().len(), 1);

    let applied = fabric.proposal_apply(id, Some("ollama")).unwrap();
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["status"], "applied");

    let updated_content = std::fs::read_to_string(vault.join("projects/demo.md")).unwrap();
    assert!(updated_content.contains("Proposed addition."));

    let _ = std::fs::remove_dir_all(&tmp);
}
