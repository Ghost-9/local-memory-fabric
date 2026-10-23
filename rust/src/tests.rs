use crate::db::busy_message;
use crate::exit_code_for;
use crate::paths::safe_relpath;
use crate::policy::{apple_path_re, secret_signature};
use crate::text::tokenize;
use serde_json::{Value, json};

#[test]
fn busy_messages_are_told_apart_from_broken_indexes() {
    // Retry is right for the first, pointless for the second; both must be greppable.
    assert!(busy_message("database is locked", "dispatch").contains("busy"));
    assert!(busy_message("database is locked", "dispatch").contains("retry shortly"));
    assert!(busy_message("database is busy", "dispatch").contains("busy"));
    // A genuinely broken index must NOT be dressed up as transient contention.
    let broken = busy_message("database disk image is malformed", "dispatch");
    assert!(
        !broken.contains("busy"),
        "malformed index misreported as busy"
    );
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
        exit_code_for(
            &json!({"ok": true, "results": [], "abstention": "no indexed source contains these terms"})
        ),
        1
    );
    assert_eq!(
        exit_code_for(
            &json!({"ok": true, "results": [], "shown": 0, "total_matches": 0, "abstention": null})
        ),
        1
    );
    // A policy refusal is its own outcome, not a failure of the request.
    assert_eq!(
        exit_code_for(
            &json!({"ok": false, "blocked": "Proposal contents are available only to an explicitly allowed local model provider."})
        ),
        2
    );
    // A real error is 3.
    assert_eq!(
        exit_code_for(&json!({"ok": false, "error": "invalid project identifier"})),
        3
    );
}

#[test]
fn safe_paths_reject_traversal() {
    assert!(safe_relpath("../secret.md").is_err());
    assert_eq!(safe_relpath("projects/a.md").unwrap(), "projects/a.md");
}

#[test]
fn mcp_parameter_coercion_and_file_uri_anchors_match_adapter_contract() {
    assert_eq!(crate::mcp_integer(&json!("1200"), 0), 1200);
    assert_eq!(crate::mcp_integer(&json!(15.5), 0), 15);
    assert_eq!(crate::mcp_integer(&Value::Null, 42), 42);
    assert_eq!(crate::mcp_integer(&json!("not_a_number"), 10), 10);
    assert_eq!(crate::mcp_string(&json!(123), ""), "123");
    assert_eq!(crate::mcp_string(&Value::Null, "default"), "default");
    assert!(crate::mcp_boolean(&json!("yes"), false));
    assert!(crate::mcp_boolean(&json!("1"), false));
    assert!(!crate::mcp_boolean(&json!("false"), true));
    assert!(!crate::mcp_boolean(&Value::Null, false));
    let uri =
        crate::anchored_uri(std::path::Path::new("/tmp/vault"), "projects/atlas.md", 25).unwrap();
    assert!(uri.starts_with("file:///"));
    assert!(uri.ends_with("#L25"));
    assert!(
        crate::anchored_uri(std::path::Path::new("/tmp/vault"), "note.md", 0)
            .unwrap()
            .ends_with("#L1")
    );
}
#[test]
fn tokenization_is_casefolded_and_deduplicated() {
    // Retained tokenizer contract: tokens("SQLite sqlite FTS5 the") == ["fts5","sqlite"].
    // `SQLite` splits to [sq, lite] and `FTS5` to [fts, 5]; after dropping sub-3-char parts only
    // ONE part remains in each case, so neither is compound and no partial token is indexed.
    // That filter-then-test ordering is what keeps the engines identical - see
    // `camel_split_preserves_identifier_boundaries` covers the cases that DO split.
    let got: Vec<String> = tokenize("SQLite sqlite FTS5 the").into_iter().collect();
    assert_eq!(got, vec!["fts5", "sqlite"]);
    assert!(
        got.contains(&"sqlite".to_string()),
        "original token must survive splitting"
    );
    assert!(
        got.contains(&"fts5".to_string()),
        "original token must survive splitting"
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
fn secret_filter_catches_openai_project_keys_without_flagging_task_identifiers() {
    let credential = format!("sk-proj-{}", "A".repeat(24));
    assert_eq!(secret_signature(&credential), Some("openai_api_key"));
    assert_eq!(
        secret_signature("Task identifiers can contain task- words without credentials."),
        None
    );
}
#[test]
fn apple_paths_are_rejected() {
    assert!(apple_path_re().is_match("Apple Notes Export"));
}

#[test]
fn camel_split_preserves_identifier_boundaries() {
    use crate::text::split_identifier;
    // These are the retained identifier-splitting expectations, minus parts shorter than
    // 3 characters (which the caller filters).
    assert_eq!(
        split_identifier("CliffCompaction"),
        vec!["cliff", "compaction"]
    );
    assert_eq!(split_identifier("parseNotes"), vec!["parse", "notes"]);
    assert_eq!(
        split_identifier("localMemoryFabric"),
        vec!["local", "memory", "fabric"]
    );
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
    std::fs::write(
        vault.join("projects/demo.md"),
        "# Project Demo\n\n## Overview\nInitial content.\n",
    )
    .unwrap();
    let fabric = crate::MemoryFabric::with_paths(home.clone(), vault.clone()).unwrap();
    fabric.index(true).unwrap();

    let list_empty = fabric.proposal_list().unwrap();
    assert_eq!(list_empty["ok"], true);
    assert_eq!(list_empty["proposals"].as_array().unwrap().len(), 0);

    let prop = fabric
        .propose(&json!({
            "project": "demo",
            "target": "projects/demo.md",
            "section": "Overview",
            "text": "Proposed addition.",
            "provider": "ollama"
        }))
        .unwrap();
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

    assert_eq!(
        server_mapping
            .ctrl()
            .status
            .load(std::sync::atomic::Ordering::Acquire),
        crate::shm::STATUS_REQ_PENDING
    );
    assert_eq!(server_mapping.req_buf(), req_payload);

    let resp_payload = b"{\"ok\":true,\"documents\":10}";
    server_mapping.write_resp(resp_payload, 0).unwrap();

    assert_eq!(
        client_mapping
            .ctrl()
            .status
            .load(std::sync::atomic::Ordering::Acquire),
        crate::shm::STATUS_RESP_READY
    );
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
        assert_eq!(
            server_mapping
                .ctrl()
                .status
                .load(std::sync::atomic::Ordering::Acquire),
            crate::shm::STATUS_REQ_PENDING
        );
        server_mapping.write_resp(resp_payload, 0).unwrap();
        assert_eq!(
            client_mapping
                .ctrl()
                .status
                .load(std::sync::atomic::Ordering::Acquire),
            crate::shm::STATUS_RESP_READY
        );
        client_mapping.ctrl().status.store(
            crate::shm::STATUS_IDLE,
            std::sync::atomic::Ordering::Release,
        );
    }
    let elapsed = start.elapsed();
    let per_op_micros = (elapsed.as_nanos() as f64 / n as f64) / 1000.0;
    println!(
        "SHM 10,000 round-trips: {:?} total, {:.3} µs per op",
        elapsed, per_op_micros
    );
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

    assert!(
        (sim_same - 1.0).abs() < 1e-4,
        "identical text should have cosine sim ~ 1.0, got {}",
        sim_same
    );
    assert!(
        sim_diff < 0.3,
        "unrelated text should have low cosine sim, got {}",
        sim_diff
    );

    // Test RRF-60
    let rrf_top = crate::semantic::rrf_score(1, 1);
    let rrf_low = crate::semantic::rrf_score(10, 10);
    assert!(
        rrf_top > rrf_low,
        "top ranks should yield higher RRF-60 score"
    );
    assert!((rrf_top - (1.0 / 61.0 + 1.0 / 61.0)).abs() < 1e-6);

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

    std::fs::write(
        primary_vault.join("projects/local.md"),
        "# Local note

Local content unique to primary.
",
    )
    .unwrap();
    std::fs::write(
        peer_vault.join("projects/peer.md"),
        "# Peer note

Peer content in worktree.
",
    )
    .unwrap();

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
    let res = fab
        .federate_search(
            "content",
            "all",
            10,
            &peers,
            Some("ollama"),
            None,
            Some(true),
            None,
        )
        .unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["federated"], true);
    let results = res["results"].as_array().unwrap();
    assert!(
        results.len() >= 2,
        "federated search should return results from both primary and peer vaults"
    );

    // Check vault_origin tags
    let origins: Vec<&str> = results
        .iter()
        .filter_map(|r| r["vault_origin"].as_str())
        .collect();
    assert!(
        origins.contains(&"primary"),
        "should contain primary origin"
    );
    assert!(
        origins.iter().any(|o| *o != "primary"),
        "should contain peer origin"
    );

    // 3. Merge proposal for new note from peer
    let merge_prop = fab
        .federate_merge_proposal(
            &peer_vault.to_string_lossy(),
            "projects/peer.md",
            Some("ollama"),
        )
        .unwrap();
    println!("merge_prop: {}", merge_prop);
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
    std::fs::write(
        vault.join("projects/atlas-ep1.md"),
        "# Atlas Note 1

Database connection pool size set to 25.
",
    )
    .unwrap();
    std::fs::write(
        vault.join("projects/atlas-ep2.md"),
        "# Atlas Note 2

Redis cache TTL configured for 300 seconds.
",
    )
    .unwrap();

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
    use crate::fuzzy::{bitset_intersects, fuzzy_filter_sections, masked_fuzzy_score};
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
    assert!(bitset_intersects(
        &["memory", "fabric"],
        &["local", "memory", "fabric"]
    ));
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
    use crate::symbiosis::{calculate_path_proximity, extract_symbols};

    let sample_text =
        "See `MemoryFabric` in [retrieval.rs](rust/src/retrieval.rs#L40) and [[compact.rs]].";
    let symbols = extract_symbols(sample_text);
    assert!(
        symbols
            .iter()
            .any(|s| s == "MemoryFabric" || s == "rust/src/retrieval.rs" || s == "compact.rs")
    );

    let p_identical =
        calculate_path_proximity(Some("rust/src/retrieval.rs"), "rust/src/retrieval.rs");
    assert_eq!(p_identical, 1.0);

    let p_sibling = calculate_path_proximity(Some("rust/src/retrieval.rs"), "rust/src/main.rs");
    assert!(p_sibling > 0.5);

    let p_distant = calculate_path_proximity(Some("rust/src/retrieval.rs"), "docs/spec.md");
    assert!(p_sibling > p_distant);
}

#[test]
fn test_harvest_transcript_and_failure_detection() {
    use crate::harvest::SessionHarvester;
    use std::io::Write;

    let tmp_dir = std::env::temp_dir().join(format!("mf-harvest-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let session_file = tmp_dir.join("test_session.jsonl");

    let mut f = std::fs::File::create(&session_file).unwrap();
    writeln!(f, "{{\"role\": \"user\", \"content\": \"Remember to always verify tests before committing.\"}}").unwrap();
    writeln!(
        f,
        "{{\"role\": \"tool\", \"content\": \"Command exited with code 1. Fatal: syntax error.\"}}"
    )
    .unwrap();

    let candidates = SessionHarvester::harvest_single_transcript(&session_file).unwrap();
    assert_eq!(
        candidates.len(),
        2,
        "Harvester must extract user correction and tool failure"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_ebbinghaus_temporal_decay() {
    use crate::retrieval::calculate_ebbinghaus_decay;

    let inv = calculate_ebbinghaus_decay("invariants", "invariants/test.md", None);
    assert_eq!(inv, 1.0, "Invariants must have zero decay");

    let user_note = calculate_ebbinghaus_decay("user", "user/prefs.md", None);
    assert_eq!(user_note, 1.0, "User notes must have zero decay");

    let thirty_days_ago = chrono::Utc::now() - chrono::Duration::days(30);
    let decay_episodic = calculate_ebbinghaus_decay(
        "episodic",
        "episodes/2026-09-01.md",
        Some(&thirty_days_ago.to_rfc3339()),
    );
    assert!(
        (decay_episodic - 0.5).abs() < 0.05,
        "30 days must be approx 0.5 half-life decay"
    );
}

#[test]
fn test_semantic_contradiction_detection() {
    use crate::conflicts::ConflictDetector;

    let tmp_dir = std::env::temp_dir().join(format!("mf-conflict-test-{}", std::process::id()));
    let inv_dir = tmp_dir.join("invariants");
    let _ = std::fs::create_dir_all(&inv_dir);

    std::fs::write(
        inv_dir.join("rule_a.md"),
        "# Rules\n\n- We must always use vitest for fast testing.\n",
    )
    .unwrap();
    std::fs::write(
        inv_dir.join("rule_b.md"),
        "# Rules\n\n- We must always use jest for enterprise coverage.\n",
    )
    .unwrap();

    let scan = ConflictDetector::scan_vault_conflicts(&tmp_dir).unwrap();
    assert!(scan["conflicts_found"].as_u64().unwrap_or(0) >= 1);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_automated_status_rolling() {
    use crate::roll::MemoryRoller;

    let tmp_dir = std::env::temp_dir().join(format!("mf-roll-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let mut lines: Vec<String> = Vec::new();
    lines.push("# Project Status Tracker\n".to_string());
    for i in 1..=20 {
        lines.push(format!("- [x] Task {} completed and deployed\n", i));
    }
    for i in 1..=10 {
        lines.push(format!("- [ ] Active task {} in progress\n", i));
    }
    std::fs::write(tmp_dir.join("STATUS.md"), lines.concat()).unwrap();

    let roll_res = MemoryRoller::roll_file(&tmp_dir, "STATUS.md", 15, false).unwrap();
    assert!(roll_res["rolled"].as_bool().unwrap_or(false));
    assert_eq!(roll_res["archived_items"].as_u64().unwrap_or(0), 20);

    let updated = std::fs::read_to_string(tmp_dir.join("STATUS.md")).unwrap();
    assert!(!updated.contains("- [x] Task 1 completed"));
    assert!(updated.contains("- [ ] Active task 1"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_binary_ast_index_compilation_and_lookup() {
    use crate::ast_cache::AstBinaryIndex;

    let tmp_dir = std::env::temp_dir().join(format!("mf-ast-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let sample_doc = "# Architecture\n\nIntro paragraph\n\n## Data Model\n\nSchema details\n\n## Security Boundaries\n\nAuth info\n";
    std::fs::write(tmp_dir.join("arch.md"), sample_doc).unwrap();

    let bin_path = tmp_dir.join("index.bin");
    let comp = AstBinaryIndex::compile(&tmp_dir, &bin_path).unwrap();
    assert!(comp["ok"].as_bool().unwrap_or(false));

    let bounds = AstBinaryIndex::lookup(&bin_path, "arch.md", "Security Boundaries");
    assert!(bounds.is_some());

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_merkle_audit_log_and_tamper_detection() {
    use crate::governance::GovernanceEngine;

    let tmp_dir = std::env::temp_dir().join(format!("mf-audit-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let log_path = tmp_dir.join("audit.log");

    GovernanceEngine::append_audit_entry(&log_path, "create", "user/prefs.md", "agent", "initial")
        .unwrap();
    GovernanceEngine::append_audit_entry(&log_path, "update", "user/prefs.md", "user", "updated")
        .unwrap();

    let verify_clean = GovernanceEngine::verify_audit_log(&log_path).unwrap();
    assert!(verify_clean["ok"].as_bool().unwrap_or(false));
    assert_eq!(verify_clean["entries_verified"].as_u64().unwrap_or(0), 2);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_predictive_cache_prewarming() {
    use crate::governance::GovernanceEngine;

    let tmp_dir = std::env::temp_dir().join(format!("mf-prewarm-test-{}", std::process::id()));
    let inv_dir = tmp_dir.join("invariants");
    let _ = std::fs::create_dir_all(&inv_dir);

    std::fs::write(inv_dir.join("rust-invariants.md"), "# Rust Rules\n").unwrap();
    std::fs::write(inv_dir.join("typescript-invariants.md"), "# TS Rules\n").unwrap();

    let files = vec!["src/main.rs".to_string(), "frontend/app.tsx".to_string()];
    let res = GovernanceEngine::prewarm_cache(&tmp_dir, &files).unwrap();
    assert!(res["ok"].as_bool().unwrap_or(false));
    assert_eq!(res["prewarmed_cards_count"].as_u64().unwrap_or(0), 2);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
