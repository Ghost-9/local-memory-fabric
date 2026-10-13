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

#[test]
fn shm_ring_buffer_roundtrip() {
    let tmp = std::env::temp_dir().join(format!("mf-shm-test-{}", uuid::Uuid::new_v4().simple()));
    let server_mapping = crate::shm::ShmMapping::open_or_create(&tmp, true).unwrap();
    let client_mapping = crate::shm::ShmMapping::open_or_create(&tmp, false).unwrap();

    let req_payload = b"{\"op\":\"status\"}";
    client_mapping.write_req(req_payload).unwrap();

    assert_eq!(server_mapping.ctrl().status.load(std::sync::atomic::Ordering::Acquire), crate::shm::STATUS_REQ_PENDING);
    assert_eq!(server_mapping.req_buf(), req_payload);

    let resp_payload = b"{\"ok\":true,\"documents\":10}";
    server_mapping.write_resp(resp_payload, 0).unwrap();

    assert_eq!(client_mapping.ctrl().status.load(std::sync::atomic::Ordering::Acquire), crate::shm::STATUS_RESP_READY);
    assert_eq!(client_mapping.resp_buf(), resp_payload);

    drop(server_mapping);
    drop(client_mapping);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn shm_benchmark_throughput() {
    let tmp = std::env::temp_dir().join(format!("mf-shm-bench-{}", uuid::Uuid::new_v4().simple()));
    let server_mapping = crate::shm::ShmMapping::open_or_create(&tmp, true).unwrap();
    let client_mapping = crate::shm::ShmMapping::open_or_create(&tmp, false).unwrap();

    let req_payload = b"{\"op\":\"status\"}";
    let resp_payload = b"{\"ok\":true}";

    let start = std::time::Instant::now();
    let n = 10_000;
    for _ in 0..n {
        client_mapping.write_req(req_payload).unwrap();
        assert_eq!(server_mapping.ctrl().status.load(std::sync::atomic::Ordering::Acquire), crate::shm::STATUS_REQ_PENDING);
        server_mapping.write_resp(resp_payload, 0).unwrap();
        assert_eq!(client_mapping.ctrl().status.load(std::sync::atomic::Ordering::Acquire), crate::shm::STATUS_RESP_READY);
        client_mapping.ctrl().status.store(crate::shm::STATUS_IDLE, std::sync::atomic::Ordering::Release);
    }
    let elapsed = start.elapsed();
    let per_op_micros = (elapsed.as_nanos() as f64 / n as f64) / 1000.0;
    println!("SHM 10,000 round-trips: {:?} total, {:.3} µs per op", elapsed, per_op_micros);
    assert!(per_op_micros < 5.0, "SHM IPC took more than 5 µs per op");
    drop(server_mapping);
    drop(client_mapping);
    let _ = std::fs::remove_file(&tmp);
}


#[test]
fn test_semantic_quantized_vector_and_rrf() {
    let v1 = crate::semantic::quantize_vector("prefix cache stability in LLM tokens");
    let v2 = crate::semantic::quantize_vector("prefix cache stability in LLM tokens");
    let v3 = crate::semantic::quantize_vector("completely unrelated cooking recipes soup");

    let sim_same = crate::semantic::cosine_similarity(&v1, &v2);
    let sim_diff = crate::semantic::cosine_similarity(&v1, &v3);

    assert!((sim_same - 1.0).abs() < 1e-4, "identical text should have cosine sim ~ 1.0, got {}", sim_same);
    assert!(sim_diff < 0.3, "unrelated text should have low cosine sim, got {}", sim_diff);

    // Test RRF-60
    let rrf_top = crate::semantic::rrf_score(1, 1);
    let rrf_low = crate::semantic::rrf_score(10, 10);
    assert!(rrf_top > rrf_low, "top ranks should yield higher RRF-60 score");
    assert!((rrf_top - (1.0/61.0 + 1.0/61.0)).abs() < 1e-6);

    let mut terms = std::collections::BTreeSet::new();
    terms.insert("forgets".to_string());
    let expanded = crate::semantic::expand_semantic_terms(&terms);
    assert!(expanded.contains(&"prefix".to_string()));
    assert!(expanded.contains(&"cache".to_string()));
}


#[test]
fn test_federate_cross_worktree_multi_vault() {
    let tmp = std::env::temp_dir().join(format!("mf-fed-{}", uuid::Uuid::new_v4().simple()));
    let primary_home = tmp.join("primary-home");
    let primary_vault = tmp.join("primary-vault");
    let peer_home = tmp.join("peer-home");
    let peer_vault = tmp.join("peer-vault");

    std::fs::create_dir_all(primary_vault.join("projects")).unwrap();
    std::fs::create_dir_all(peer_vault.join("projects")).unwrap();

    std::fs::write(primary_vault.join("projects/local.md"), "# Local note

Local content unique to primary.
").unwrap();
    std::fs::write(peer_vault.join("projects/peer.md"), "# Peer note

Peer content in worktree.
").unwrap();

    let fab = crate::MemoryFabric::with_paths(primary_home, primary_vault.clone()).unwrap();
    fab.index(true).unwrap();

    let peer_fab = crate::MemoryFabric::with_paths(peer_home, peer_vault.clone()).unwrap();
    peer_fab.index(true).unwrap();

    // 1. Status
    let st = fab.federate_status().unwrap();
    assert_eq!(st["ok"], true);
    assert_eq!(st["federated_enabled"], true);

    // 2. Federated Search
    let peers = vec![peer_vault.to_string_lossy().to_string()];
    let res = fab.federate_search("content", "all", 10, &peers, Some("ollama"), None, Some(true), None).unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["federated"], true);
    let results = res["results"].as_array().unwrap();
    assert!(results.len() >= 2, "federated search should return results from both primary and peer vaults");

    // Check vault_origin tags
    let origins: Vec<&str> = results.iter().filter_map(|r| r["vault_origin"].as_str()).collect();
    assert!(origins.contains(&"primary"), "should contain primary origin");
    assert!(origins.iter().any(|o| *o != "primary"), "should contain peer origin");

    // 3. Merge proposal for new note from peer
    let merge_prop = fab.federate_merge_proposal(&peer_vault.to_string_lossy(), "projects/peer.md", Some("ollama")).unwrap(); println!("merge_prop: {}", merge_prop);
    assert_eq!(merge_prop["ok"], true);

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}


#[test]
fn test_autonomous_knowledge_distillation_and_compactor() {
    let tmp = std::env::temp_dir().join(format!("mf-compact-{}", uuid::Uuid::new_v4().simple()));
    let home = tmp.join("state");
    let vault = tmp.join("vault");

    std::fs::create_dir_all(vault.join("projects")).unwrap();
    std::fs::write(vault.join("projects/atlas-ep1.md"), "# Atlas Note 1

Database connection pool size set to 25.
").unwrap();
    std::fs::write(vault.join("projects/atlas-ep2.md"), "# Atlas Note 2

Redis cache TTL configured for 300 seconds.
").unwrap();

    let fab = crate::MemoryFabric::with_paths(home, vault.clone()).unwrap();
    fab.index(true).unwrap();

    let res = fab.compact(Some("atlas"), Some("ollama")).unwrap();
    assert_eq!(res["ok"], true);
    assert!(res["compacted_count"].as_u64().unwrap() >= 2);
    assert_eq!(res["invariant_card"], "projects/atlas-invariants.md");

    // Verify invariant file was created with supersedes links
    let inv_path = vault.join("projects/atlas-invariants.md");
    assert!(inv_path.exists());
    let inv_content = std::fs::read_to_string(inv_path).unwrap();
    assert!(inv_content.contains("Supersedes:"));
    assert!(inv_content.contains("[[projects/atlas-ep1.md]]"));
    assert!(inv_content.contains("Database connection pool"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}


#[test]
fn test_fsearch_masked_fuzzy_engine_and_bitset() {
    use crate::fuzzy::{masked_fuzzy_score, bitset_intersects, fuzzy_filter_sections};
    use crate::text::MarkdownSection;

    // Bitmask test
    let pat = "arch";
    let tgt = "architecture-overview";
    let score = masked_fuzzy_score(pat, tgt);
    assert!(score.is_some());
    assert!(score.unwrap() > 0.5);

    // Rejection on disjoint characters
    assert!(masked_fuzzy_score("xyz", tgt).is_none());

    // Word boundary match bonus
    let abbr_score = masked_fuzzy_score("ao", tgt);
    assert!(abbr_score.is_some());

    // Bitset token intersection
    assert!(bitset_intersects(&["memory", "fabric"], &["local", "memory", "fabric"]));
    assert!(!bitset_intersects(&["rust"], &["python", "typescript"]));

    // Section filter
    let sections = vec![
        MarkdownSection {
            heading: "System Architecture".into(),
            anchor: "system-architecture".into(),
            level: 2,
            line_start: 1,
            line_end: 10,
            char_start: 0,
            char_end: 100,
            breadcrumbs: vec!["Root".into(), "System Architecture".into()],
        },
        MarkdownSection {
            heading: "Data Pipeline".into(),
            anchor: "data-pipeline".into(),
            level: 2,
            line_start: 11,
            line_end: 20,
            char_start: 101,
            char_end: 200,
            breadcrumbs: vec!["Root".into(), "Data Pipeline".into()],
        },
    ];

    let filtered = fuzzy_filter_sections("arch", &sections);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0.heading, "System Architecture");
}

#[test]
fn test_memory_code_symbiosis_and_proximity_reranking() {
    use crate::symbiosis::{extract_symbols, calculate_path_proximity};

    let sample_text = "See `MemoryFabric` in [retrieval.rs](rust/src/retrieval.rs#L40) and [[compact.rs]].";
    let symbols = extract_symbols(sample_text);
    assert!(symbols.iter().any(|s| s == "MemoryFabric" || s == "rust/src/retrieval.rs" || s == "compact.rs"));

    let p_identical = calculate_path_proximity(Some("rust/src/retrieval.rs"), "rust/src/retrieval.rs");
    assert_eq!(p_identical, 1.0);

    let p_sibling = calculate_path_proximity(Some("rust/src/retrieval.rs"), "rust/src/main.rs");
    assert!(p_sibling > 0.5);

    let p_distant = calculate_path_proximity(Some("rust/src/retrieval.rs"), "docs/spec.md");
    assert!(p_sibling > p_distant);
}
