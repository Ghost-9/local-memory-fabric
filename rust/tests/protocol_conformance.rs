use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use uuid::Uuid;

const BIN: &str = env!("CARGO_BIN_EXE_memory-fabric");

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    vault: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("engram-conformance-{}", Uuid::new_v4()));
        let home = root.join("state");
        let vault = root.join("vault");
        fs::create_dir_all(vault.join("projects")).unwrap();
        fs::create_dir_all(vault.join("company")).unwrap();
        fs::create_dir_all(vault.join("pi-harness")).unwrap();
        fs::create_dir_all(vault.join("Apple Notes")).unwrap();
        Self { root, home, vault }
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.vault.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("HOME", &self.root)
            .env("MEMORY_FABRIC_HOME", &self.home)
            .env("MEMORY_FABRIC_VAULT", &self.vault)
            .env_remove("PI_MEMORY_LOCAL_PROVIDERS")
            .env_remove("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS")
            .env_remove("PI_MEMORY_REMOTE_FULL_ACCESS")
            .env_remove("PI_MEMORY_REMOTE_CONTEXT_SCRUB");
        command
    }

    fn ask_with(&self, req: &Value, envs: &[(&str, &str)]) -> Value {
        let mut command = self.command(&[]);
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(child.stdin.as_mut().unwrap(), req).unwrap();
        child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        serde_json::from_str(&stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON ({e}): {stdout}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    fn ask(&self, req: &Value) -> Value {
        self.ask_with(req, &[])
    }

    fn ask_many(&self, args: &[&str], requests: &[Value], envs: &[(&str, &str)]) -> Vec<Value> {
        let mut command = self.command(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            for req in requests {
                serde_json::to_writer(&mut *stdin, req).unwrap();
                stdin.write_all(b"\n").unwrap();
            }
        }
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "server failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn index(&self) -> Value {
        self.ask(&json!({"op":"index","force":true}))
    }

    fn local(&self, mut req: Value) -> Value {
        req["provider"] = json!("ollama");
        req["loopback"] = json!(true);
        self.ask(&req)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn probe_corpus(f: &Fixture) {
    f.write("projects/atlas.md", "---\nstatus: current\nclassification: local_only\nvalid_from: 2024-01-01\n---\n\n# Atlas project\n\n## Decisions\n\nThe local retrieval service uses SQLite FTS5 and preserves source provenance.\n\n## Log\n\n- First entry.\n");
    f.write("projects/atlas/legacy.md", "---\nstatus: superseded\nvalid_from: 2020-01-01\nvalid_to: 2024-01-01\nsuperseded_by: projects/atlas.md\n---\n\n# Legacy Atlas retention\n\nThe historic retention snapshots used the retired vault format.\n");
    f.write("projects/atlas/current-policy.md", "---\nstatus: current\nvalid_from: 2022-01-01\n---\n\n# Current Atlas policy\n\nCurrent retention snapshots replaced the old archive rules.\n");
    f.write("pi-harness/cache.md", "# Prompt cache behavior\n\n## Cache\n\nChanging an early prompt section can invalidate the cached conversation prefix. Append context to preserve reuse.\n");
    f.write("company/policy.md", "# Company privacy policy\n\n## Rules\n\nDurable memory changes require human review and approval.\n");
}

#[test]
fn provider_policy_remote_scrubs_by_default_is_propose_only_and_opt_out_is_explicit() {
    let f = Fixture::new();
    let request = json!({"op":"provider-policy","model":{"provider":"OPENAI","base_url":"https://api.openai.com/v1"}});
    let default = f.ask_with(
        &request,
        &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")],
    );
    assert_eq!(default["allowed"], true);
    assert_eq!(default["loopback"], false);
    assert_eq!(default["scrub_context"], true);
    assert_eq!(default["may_write"], false);
    assert_eq!(default["write_mode"], "propose_only");

    let opted_out = f.ask_with(
        &request,
        &[
            ("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai"),
            ("PI_MEMORY_REMOTE_CONTEXT_SCRUB", "0"),
        ],
    );
    assert_eq!(opted_out["allowed"], true);
    assert_eq!(opted_out["scrub_context"], false);
    assert_eq!(opted_out["may_write"], false);

    let full = f.ask_with(
        &request,
        &[
            ("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai"),
            ("PI_MEMORY_REMOTE_FULL_ACCESS", "1"),
        ],
    );
    assert_eq!(full["may_write"], true);
    let unlisted = f.ask_with(&json!({"op":"provider-policy","model":{"provider":"anthropic","base_url":"https://api.anthropic.com"}}), &[("PI_MEMORY_REMOTE_FULL_ACCESS", "1")]);
    assert_eq!(unlisted["allowed"], false);
    assert_eq!(unlisted["may_write"], false);

    let local = f.ask(&json!({"op":"provider-policy","model":{"provider":"ollama","base_url":"http://127.0.0.1:11434"}}));
    assert_eq!(local["allowed"], true);
    assert_eq!(local["loopback"], true);
    assert_eq!(local["scrub_context"], false);
    assert_eq!(local["may_write"], true);
}

#[test]
fn runtime_version_is_sourced_from_cargo_package_metadata() {
    let f = Fixture::new();
    let output = f.command(&["--version"]).output().unwrap();
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout).unwrap();
    assert!(
        version.starts_with(&format!(
            "Engram / Local Memory Fabric {} (schema ",
            env!("CARGO_PKG_VERSION")
        )),
        "runtime version must match Cargo.toml: {version}"
    );
    assert!(version.ends_with(")\n"));
}

#[test]
fn tool_guard_blocks_remote_shell_mutation_inside_the_vault() {
    let f = Fixture::new();
    let model = json!({"provider":"openai","base_url":"https://api.openai.com/v1"});
    let envs = &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")];
    let blocked = f.ask_with(
        &json!({
            "op":"tool-guard",
            "model":model,
            "cwd":f.vault.to_string_lossy(),
            "tool_name":"bash",
            "input":{"command":"rm projects/atlas.md"}
        }),
        envs,
    );
    assert_eq!(blocked["block"], true);
    assert!(blocked["reason"].as_str().unwrap().contains("read-only"));

    let read_only = f.ask_with(
        &json!({
            "op":"tool-guard",
            "model":model,
            "cwd":f.vault.to_string_lossy(),
            "tool_name":"bash",
            "input":{"command":"ls projects"}
        }),
        envs,
    );
    assert_eq!(read_only["block"], false);
}

#[test]
fn documented_rust_policy_operations_are_available_through_jsonl_dispatch() {
    let f = Fixture::new();
    let requests = vec![
        json!({"op":"provider-policy","session_id":"policy-ops","model":{"provider":"openai","base_url":"https://api.openai.com/v1"}}),
        json!({"op":"write-policy","session_id":"policy-ops","model":{"provider":"openai","base_url":"https://api.openai.com/v1"}}),
        json!({"op":"scrub-context","session_id":"policy-ops","model":{"provider":"openai","base_url":"https://api.openai.com/v1"},"messages":[{"role":"assistant","provider":"ollama","content":"local-only turn"}],"now_ms":1}),
        json!({"op":"injection-decision","prompt":"temporal supersession","response":{"results":[{"matched_terms":["temporal","supersession"]}]}}),
        json!({"op":"abstention-decision","query":"temporal supersession","evidence":{"results":[{"matched_terms":["temporal"]}]},"merge":true}),
        json!({"op":"scope-policy","action":"tier","relpath":"company/policy.md"}),
        json!({"op":"scope-policy","action":"project-match","relpath":"projects/local-memory-fabric-extension.md","cwd":"/work/local-memory-fabric"}),
        json!({"op":"scope-policy","action":"handoff","cwd":"/work/atlas","handoff":{"cwd":"/work/atlas"}}),
        json!({"op":"t3-adapter","action":"resolve-workspace","workspace_path":"/work/My Project"}),
        json!({"op":"t3-adapter","action":"instruction","project":"my-project"}),
        json!({"op":"session-cost","session_id":"policy-ops","threshold":1,"interval":3,"usage":{"input":2_000}}),
        json!({"op":"resume-pointer","handoff":{"at":"2026-10-23T00:00:00Z","next":"run gate"},"now_ms":1_792_800_000_000_i64}),
        json!({"op":"engine-health","session_id":"policy-ops","action":"status","now_ms":100}),
        json!({"op":"injection-account","session_id":"policy-ops","action":"record","chars":120,"refs":2}),
        json!({"op":"injection-account","session_id":"policy-ops","action":"state"}),
        json!({"op":"scrub-text","text":"api_key=supersecret123456"}),
    ];
    let responses = f.ask_many(
        &["--serve"],
        &requests,
        &[
            ("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai"),
            ("PI_MEMORY_REMOTE_FULL_ACCESS", "0"),
        ],
    );
    assert_eq!(responses.len(), requests.len());
    assert_eq!(responses[0]["allowed"], true);
    assert_eq!(responses[1]["write_mode"], "propose_only");
    assert_eq!(responses[2]["removed_anything"], true);
    assert_eq!(responses[3]["inject"], true);
    assert_eq!(responses[4]["abstained"], false);
    assert_eq!(responses[5]["tier"], "global");
    assert_eq!(responses[6]["matches"], true);
    assert_eq!(responses[7]["may_inject"], true);
    assert_eq!(responses[8]["project"], "my-project");
    assert!(
        responses[9]["instruction"]
            .as_str()
            .unwrap()
            .contains("[PROJECT: my-project]")
    );
    assert_eq!(responses[10]["advise"], true);
    assert!(
        responses[10]["notice"]
            .as_str()
            .unwrap()
            .contains("memory_fabric_handoff")
    );
    assert!(
        responses[11]["pointer"]
            .as_str()
            .unwrap()
            .contains("run gate")
    );
    assert!(responses[12]["state"].is_object());
    assert_eq!(responses[13]["turn"], 1);
    assert_eq!(responses[14]["baselineChars"], 120);
    assert!(
        !responses[15]["text"]
            .as_str()
            .unwrap()
            .contains("supersecret123456")
    );
    assert!(responses[15]["text"].as_str().unwrap().contains("REDACTED"));
}

#[test]
fn configured_local_provider_is_the_same_allowlist_for_dispatch_and_policy_ops() {
    let f = Fixture::new();
    fs::create_dir_all(&f.home).unwrap();
    fs::write(
        f.home.join("config.json"),
        r#"{"local_providers":["desktop-ai"]}"#,
    )
    .unwrap();
    let model = json!({"provider":"DESKTOP-AI","base_url":"http://localhost:1234"});
    let decision = f.ask(&json!({"op":"provider-policy","model":model}));
    assert_eq!(decision["allowed"], true);
    assert_eq!(decision["loopback"], true);
    assert_eq!(decision["may_write"], true);
    f.write(
        "projects/note.md",
        "# Note\n\ncustom provider configuration is honored.\n",
    );
    let indexed = f.ask(&json!({"op":"index"}));
    assert_eq!(indexed["ok"], true);
    let search = f.ask(&json!({"op":"search","query":"provider configuration","provider":"desktop-ai","loopback":true}));
    assert_eq!(search["ok"], true);
    assert!(!search["results"].as_array().unwrap().is_empty());
}

#[test]
fn allowlisted_remote_transcript_scrubs_by_default_and_only_explicit_optout_keeps_it() {
    let f = Fixture::new();
    let model = json!({"provider":"openai","base_url":"https://api.openai.com/v1"});
    let messages = json!([
        {"role":"user","customType":"vault-context-route","content":"private routing note"},
        {"role":"user","content":"ordinary user prompt"}
    ]);
    let request = json!({"op":"scrub-context","session_id":"remote-default","model":model,"messages":messages,"now_ms":1000});
    let default = f.ask_with(
        &request,
        &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")],
    );
    assert_eq!(default["removed_anything"], true);
    assert_eq!(default["notice"], true);
    assert_eq!(default["messages"].as_array().unwrap().len(), 1);
    assert_eq!(default["messages"][0]["content"], "ordinary user prompt");
    let opted_out = f.ask_with(
        &json!({"op":"scrub-context","model":model,"messages":messages,"now_ms":1000}),
        &[
            ("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai"),
            ("PI_MEMORY_REMOTE_CONTEXT_SCRUB", "0"),
        ],
    );
    assert_eq!(opted_out["removed_anything"], false);
    assert_eq!(opted_out["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn dispatch_denies_unlisted_remote_without_returning_paths_or_memory_content() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let result = f.ask(&json!({"op":"search","query":"retrieval provenance SQLite","provider":"anthropic","base_url":"https://api.anthropic.com"}));
    assert_eq!(result["ok"], false);
    assert!(result.get("blocked").is_some());
    assert_eq!(result.as_object().unwrap().len(), 2);
    assert!(!result.to_string().to_ascii_lowercase().contains("atlas"));
    assert!(
        !result
            .to_string()
            .to_ascii_lowercase()
            .contains("retrieval")
    );
}

#[test]
fn remote_writes_are_propose_only_unless_full_access_is_explicit() {
    let f = Fixture::new();
    f.write(
        "projects/note.md",
        "# Note\n\n## Decision\nKeep the canonical Markdown note unchanged.\n",
    );
    let preview = f.ask_with(&json!({"op":"consolidate","ref":"projects/note.md","write":false,"provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["mode"], "preview");
    let direct_write = f.ask_with(&json!({"op":"consolidate","ref":"projects/note.md","write":true,"provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert!(
        direct_write.get("blocked").is_some(),
        "remote digest write must be proposal-only by default: {direct_write}"
    );
    assert!(!f.vault.join("_digests/projects__note.md").exists());
    let proposal = f.ask_with(&json!({"op":"apply","proposal_id":"missing","provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert_eq!(proposal["ok"], false);
    assert!(
        proposal.get("blocked").is_some(),
        "default remote apply must be refused before proposal lookup: {proposal}"
    );
    let opted_in = f.ask_with(&json!({"op":"apply","proposal_id":"missing","provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai"), ("PI_MEMORY_REMOTE_FULL_ACCESS", "1")]);
    assert!(
        opted_in.get("blocked").is_none(),
        "the explicit opt-in must reach normal proposal validation: {opted_in}"
    );
}

#[test]
fn init_versions_vault_and_both_init_and_doctor_explain_empty_memory() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.vault).unwrap();
    let initialized = f.ask(&json!({"op":"init"}));
    assert_eq!(initialized["ok"], true);
    assert_eq!(initialized["documents"], 0);
    assert!(
        initialized["next"]
            .as_str()
            .unwrap()
            .contains("memory returns nothing until Markdown notes exist")
    );
    assert!(
        f.vault.join(".git").is_dir(),
        "init must establish a revert point"
    );
    let doctor = f.ask(&json!({"op":"doctor"}));
    assert_eq!(doctor["ok"], false);
    assert!(
        doctor["problems"]
            .to_string()
            .contains("memory returns nothing until Markdown notes exist")
    );
}

#[test]
fn index_enforces_scope_allowlist_secret_filter_symlink_exclusion_and_private_modes() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.write("Apple Notes/not-for-index.md", "PRIVATE SENTINEL\n");
    let candidate = format!("CANDIDATE{}", "X".repeat(24));
    f.write(
        "projects/sensitive.md",
        &format!("# Sensitive\n\nSUPABASE_SERVICE_ROLE_KEY={candidate}\n"),
    );
    let outside = f.root.join("outside.md");
    fs::write(&outside, "# Outside\n\nsecret symlink target\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, f.vault.join("projects/linked.md")).unwrap();
    let result = f.index();
    assert_eq!(result["ok"], true);
    assert_eq!(result["excluded_sensitive"], 1);
    let con = Connection::open(f.home.join("index.sqlite3")).unwrap();
    let mut stmt = con.prepare("SELECT relpath,scope FROM documents").unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let docs: std::collections::HashMap<_, _> = rows.into_iter().collect();
    assert_eq!(
        docs.get("projects/atlas.md").map(String::as_str),
        Some("project:atlas")
    );
    assert_eq!(
        docs.get("company/policy.md").map(String::as_str),
        Some("company")
    );
    assert_eq!(
        docs.get("pi-harness/cache.md").map(String::as_str),
        Some("system:pi-harness")
    );
    assert!(!docs.contains_key("Apple Notes/not-for-index.md"));
    assert!(!docs.contains_key("projects/sensitive.md"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&f.home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(f.home.join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(f.home.join("index.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn scope_filter_abstention_common_terms_and_single_term_answers_are_stable() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let system = f.local(
        json!({"op":"search","query":"prompt cache early section","scope":"system:pi-harness"}),
    );
    assert_eq!(system["results"][0]["source_ref"], "pi-harness/cache.md");
    let company =
        f.local(json!({"op":"search","query":"prompt cache early section","scope":"company"}));
    assert!(company["results"].as_array().unwrap().is_empty());
    let no_hit = f.local(json!({"op":"search","query":"mars weather orbital telescope"}));
    assert_eq!(
        no_hit["abstention"],
        "no indexed source contains these terms"
    );

    let common = Fixture::new();
    for i in 0..4 {
        common.write(
            &format!("projects/n{i}.md"),
            &format!("# Note {i}\n\nalpha shared topic body number {i}\n"),
        );
    }
    common.index();
    let all = common.local(json!({"op":"search","query":"alpha shared topic"}));
    assert_eq!(all["low_discrimination"], true);
    assert!(all["shown"].as_u64().unwrap() > 0);
    assert!(all["abstention"].is_null());
    let absent = common.local(json!({"op":"search","query":"zirconium quasars"}));
    assert_eq!(absent["shown"], 0);
    assert_eq!(
        absent["abstention"],
        "no indexed source contains these terms"
    );

    let single = Fixture::new();
    for i in 0..10 {
        let text = if i < 4 {
            "provenance and lineage"
        } else {
            "unrelated material"
        };
        single.write(
            &format!("projects/n{i}.md"),
            &format!("# Note {i}\n\n{text}\n"),
        );
    }
    single.index();
    let result = single.local(json!({"op":"search","query":"provenance"}));
    assert!(result["shown"].as_u64().unwrap() > 0);
    assert!(result["abstention"].is_null());
}

#[test]
fn temporal_search_keeps_provenance_and_filters_superseded_sources_by_date() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let current = f
        .local(json!({"op":"search","query":"legacy retention snapshots","scope":"project:atlas"}));
    assert_eq!(current["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        current["results"][0]["source_ref"],
        "projects/atlas/current-policy.md"
    );
    assert!(
        current["temporal_warning"]
            .as_str()
            .unwrap()
            .contains("No as_of date supplied")
    );
    let historic = f.local(json!({"op":"search","query":"legacy retention snapshots","scope":"project:atlas","as_of":"2023-06-01"}));
    let refs: std::collections::HashSet<_> = historic["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["source_ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        refs,
        [
            "projects/atlas/legacy.md",
            "projects/atlas/current-policy.md"
        ]
        .into_iter()
        .collect()
    );
    let legacy = historic["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["source_ref"] == "projects/atlas/legacy.md")
        .unwrap();
    assert_eq!(legacy["source_status"], "superseded");
    assert_eq!(legacy["superseded_by"], "projects/atlas.md");
    assert_eq!(legacy["export_policy"], "deny_remote");
    assert!(
        historic["temporal_warning"]
            .as_str()
            .unwrap()
            .contains("Overlapping source lifecycle")
    );
    let too_early = f.local(json!({"op":"search","query":"legacy retention snapshots","scope":"project:atlas","as_of":"2019-01-01"}));
    assert!(too_early["results"].as_array().unwrap().is_empty());
    let invalid =
        f.local(json!({"op":"search","query":"legacy retention snapshots","as_of":"not-a-date"}));
    assert_eq!(invalid["ok"], false);
}

#[test]
fn export_policy_withholds_remote_notes_and_reports_the_count_but_allows_loopback() {
    let f = Fixture::new();
    for i in 0..3 {
        f.write(
            &format!("projects/n{i}.md"),
            &format!("# Note {i}\n\nalpha shared topic body {i}\n"),
        );
    }
    fs::create_dir_all(&f.home).unwrap();
    fs::write(
        f.home.join("config.json"),
        r#"{"enforce_export_policy":true,"local_providers":["ollama"]}"#,
    )
    .unwrap();
    f.index();
    let remote = f.ask_with(&json!({"op":"search","query":"alpha shared topic","provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert_eq!(remote["ok"], true);
    assert!(remote["results"].as_array().unwrap().is_empty());
    assert!(remote["withheld_export"].as_u64().unwrap() > 0);
    let local = f.local(json!({"op":"search","query":"alpha shared topic"}));
    assert_eq!(local["results"].as_array().unwrap().len(), 3);
    assert_eq!(local["withheld_export"], 0);
}

#[test]
fn export_policy_status_default_is_off_and_explicit_setting_remains_visible() {
    let off = Fixture::new();
    off.write("projects/note.md", "# Note\n\nalpha beta gamma\n");
    off.index();
    assert_eq!(
        off.ask(&json!({"op":"status"}))["enforce_export_policy"],
        false
    );

    let on = Fixture::new();
    on.write("projects/note.md", "# Note\n\nalpha beta gamma\n");
    fs::create_dir_all(&on.home).unwrap();
    fs::write(
        on.home.join("config.json"),
        r#"{"enforce_export_policy":true}"#,
    )
    .unwrap();
    on.index();
    assert_eq!(
        on.ask(&json!({"op":"status"}))["enforce_export_policy"],
        true
    );
}

#[test]
fn credential_alarms_detect_remind_redact_and_resolve_idempotently() {
    let f = Fixture::new();
    let fake = format!("ghp_{}", "A".repeat(36));
    f.write(
        "projects/creds.md",
        &format!("# Deploy\n\nSet GITHUB_TOKEN={fake} before CI.\n"),
    );
    f.write("projects/ok.md", "# Edge\n\nedge compaction needs care.\n");
    f.index();
    let alarms = f.local(json!({"op":"alarms"}));
    assert_eq!(alarms["alarms"].as_array().unwrap().len(), 1);
    let alarm = &alarms["alarms"][0];
    assert_eq!(alarm["secret_kind"], "github_token");
    assert_eq!(alarm["relpath"], "projects/creds.md");
    assert!(!alarms.to_string().contains(&fake));
    let search = f.local(json!({"op":"search","query":"edge compaction"}));
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["source_ref"] != "projects/creds.md")
    );
    assert!(
        search["credential_alarm"]
            .as_str()
            .unwrap()
            .contains("projects/creds.md")
    );
    assert!(!search["credential_alarm"].as_str().unwrap().contains(&fake));
    let context = f.local(json!({"op":"context","query":"edge compaction","token_budget":200}));
    assert!(
        context["credential_alarm"]
            .as_str()
            .unwrap()
            .contains("projects/creds.md")
    );
    assert_eq!(f.ask(&json!({"op":"status"}))["secret_alarms"], 1);

    let alarm_id = alarm["alarm_id"].as_str().unwrap();
    for bad in [
        "",
        "../../etc/passwd",
        "'; DROP TABLE secret_alarms;--",
        "ZZZZ",
    ] {
        assert_eq!(f.local(json!({"op":"resolve","alarm_id":bad}))["ok"], false);
    }
    let remote_resolve = f.ask_with(&json!({"op":"resolve","alarm_id":alarm_id,"provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert!(remote_resolve.get("blocked").is_some());
    let resolved = f.local(json!({"op":"resolve","alarm_id":alarm_id}));
    assert_eq!(resolved["ok"], true);
    assert_eq!(resolved["unresolved_remaining"], 0);
    let again = f.local(json!({"op":"resolve","alarm_id":alarm_id}));
    assert_eq!(again["ok"], true);
    assert_eq!(again["already_resolved"], true);
    assert!(
        f.local(json!({"op":"search","query":"edge compaction"}))["credential_alarm"].is_null()
    );
    assert_eq!(f.ask(&json!({"op":"status"}))["secret_alarms"], 0);
}

#[test]
fn clean_and_placeholder_notes_do_not_create_secret_alarms_or_reminder_payloads() {
    let f = Fixture::new();
    f.write("projects/clean.md", "# Clean\n\nalpha beta gamma\n");
    f.index();
    assert!(
        f.local(json!({"op":"alarms"}))["alarms"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(f.local(json!({"op":"search","query":"alpha"}))["credential_alarm"].is_null());
    assert_eq!(f.ask(&json!({"op":"status"}))["secret_alarms"], 0);

    let placeholders = Fixture::new();
    for (i, body) in [
        "api_key: <your-key-here>",
        "token = REPLACE_ME",
        "password: not-a-real-secret",
        "secret: placeholder-value",
    ]
    .iter()
    .enumerate()
    {
        placeholders.write(
            &format!("projects/note-{i}.md"),
            &format!("# Note {i}\n\n{body}\n"),
        );
    }
    placeholders.index();
    assert!(
        placeholders.local(json!({"op":"alarms"}))["alarms"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn project_context_is_local_cited_and_character_bounded() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let blocked = f.ask(&json!({"op":"context","project":"atlas","query":"SQLite FTS5 provenance","provider":"anthropic","base_url":"https://api.anthropic.com"}));
    assert_eq!(blocked["ok"], false);
    assert_eq!(blocked.as_object().unwrap().len(), 2);
    let packet = f.local(json!({"op":"context","project":"atlas","query":"SQLite FTS5 provenance","token_budget":100}));
    assert_eq!(packet["ok"], true);
    assert_eq!(packet["project"], "atlas");
    assert!(
        packet["estimated_characters"].as_u64().unwrap() <= packet["char_budget"].as_u64().unwrap()
    );
    assert!(
        packet["context"]
            .as_str()
            .unwrap()
            .contains("projects/atlas.md")
    );
    assert!(
        packet["instruction"]
            .as_str()
            .unwrap()
            .contains("source_ref")
    );
}

#[test]
fn before_agent_start_injects_only_current_project_and_global_tiers_once() {
    let f = Fixture::new();
    f.write(
        "projects/atlas.md",
        "# Atlas\n\nSQLite retrieval cache architecture decisions for Atlas.\n",
    );
    f.write(
        "projects/fikr.md",
        "# Fikr\n\nSQLite retrieval cache architecture decisions for Fikr.\n",
    );
    f.write(
        "pi-harness/cache.md",
        "# Cache\n\nSQLite retrieval cache architecture decisions for all workspaces.\n",
    );
    f.index();
    let model = json!({"provider":"ollama","base_url":"http://127.0.0.1:11434"});
    let req = json!({"op":"before-agent-start","message_type":"user","session_id":"injection-atlas","cwd":"/work/atlas","prompt":"SQLite retrieval cache architecture decisions","messages":[],"model":model,"provider":"ollama","base_url":"http://127.0.0.1:11434","loopback":true});
    let outputs = f.ask_many(
        &["--serve"],
        &[
            req.clone(),
            req,
            json!({"op":"injection-account","action":"state","session_id":"injection-atlas"}),
        ],
        &[("MEMORY_FABRIC_ENABLE_INJECTION", "1")],
    );
    let message = outputs[0]["message"]["content"].as_str().unwrap();
    assert!(message.contains("projects/atlas.md"));
    assert!(message.contains("pi-harness/cache.md"));
    assert!(!message.contains("projects/fikr.md"));
    assert!(
        outputs[1]["message"].is_null(),
        "same session must not inject a second copy"
    );
    assert_eq!(outputs[2]["turns"], 1);
    assert_eq!(outputs[2]["prefixBreaks"], 0);
    assert!(outputs[2]["sessionChars"].as_u64().unwrap() > 0);
}

#[test]
fn handoff_store_is_keyed_per_project_and_resume_is_fail_closed() {
    let f = Fixture::new();
    let model = json!({"provider":"ollama","base_url":"http://127.0.0.1:11434"});
    let base = |cwd: &str, session: &str| json!({"provider":"ollama","base_url":"http://127.0.0.1:11434","loopback":true,"model":model,"cwd":cwd,"session_id":session});
    let mut create_a = base("/work/a", "create-a");
    create_a["op"] = json!("create-handoff");
    create_a["next"] = json!("finish project A");
    let mut create_b = base("/work/b", "create-b");
    create_b["op"] = json!("create-handoff");
    create_b["next"] = json!("finish project B");
    let mut resume_a = base("/work/a", "resume-a");
    resume_a["op"] = json!("before-agent-start");
    resume_a["message_type"] = json!("user");
    resume_a["prompt"] = json!("");
    resume_a["messages"] = json!([]);
    let mut resume_b = resume_a.clone();
    resume_b["cwd"] = json!("/work/b");
    resume_b["session_id"] = json!("resume-b");
    let output = f.ask_many(
        &["--serve"],
        &[create_a, create_b, resume_a, resume_b],
        &[("MEMORY_FABRIC_ENABLE_INJECTION", "1")],
    );
    assert_eq!(output[0]["ok"], true);
    assert_eq!(output[1]["ok"], true);
    assert!(
        output[2]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("finish project A")
    );
    assert!(
        output[3]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("finish project B")
    );
    let store: Value =
        serde_json::from_slice(&fs::read(f.home.join("handoff.json")).unwrap()).unwrap();
    assert_eq!(store["/work/a"]["next"], "finish project A");
    assert_eq!(store["/work/b"]["next"], "finish project B");
    assert_eq!(store.as_object().unwrap().len(), 2);
}

#[test]
fn useful_term_cutoff_does_not_shrink_when_unrelated_notes_are_added() {
    let f = Fixture::new();
    for i in 0..6 {
        f.write(
            &format!("projects/base-{i}.md"),
            &format!("# Base {i}\n\ncompaction caching retrieval indexing ranking {i}\n"),
        );
    }
    f.index();
    let small = f.local(json!({"op":"search","query":"compaction"}))["shown"]
        .as_u64()
        .unwrap();
    for i in 6..60 {
        f.write(
            &format!("projects/extra-{i}.md"),
            &format!("# Extra {i}\n\nunrelated filler text number {i}\n"),
        );
    }
    f.index();
    let grown = f.local(json!({"op":"search","query":"compaction"}));
    assert!(grown["shown"].as_u64().unwrap() >= small);
}

#[test]
fn pre_alarm_schema_remains_readable_without_reindexing() {
    let f = Fixture::new();
    f.write(
        "projects/note.md",
        "# Note\n\nedge compaction needs care.\n",
    );
    f.index();
    let db = f.home.join("index.sqlite3");
    let con = Connection::open(&db).unwrap();
    con.execute("DROP TABLE secret_alarms", []).unwrap();
    let exists: i64 = con
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='secret_alarms'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 0);
    drop(con);
    assert_eq!(f.ask(&json!({"op":"status"}))["ok"], true);
    assert_eq!(
        f.local(json!({"op":"search","query":"edge compaction"}))["ok"],
        true
    );
    assert_eq!(
        f.local(json!({"op":"context","query":"edge compaction"}))["ok"],
        true
    );
    assert_eq!(f.local(json!({"op":"alarms"}))["ok"], true);
    assert!(
        f.local(json!({"op":"search","query":"edge compaction"}))["shown"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn sandbox_is_private_noncanonical_reindexable_and_refuses_secrets() {
    let f = Fixture::new();
    f.write("projects/n.md", "# N\n\nalpha beta\n");
    f.index();
    let payload = "shard three replicated log lines\n".repeat(400);
    let stored = f
        .local(json!({"op":"sandbox","title":"big tool output","text":payload,"scope":"episodic"}));
    assert_eq!(stored["ok"], true);
    assert_eq!(stored["chars"], payload.chars().count());
    assert!(stored["head"].as_str().unwrap().chars().count() <= 480);
    assert_eq!(stored["scope"], "episodic");
    let handle = stored["handle"].as_str().unwrap();
    let file = f
        .home
        .join("sandbox")
        .join(handle.split_once('/').unwrap().1);
    assert_eq!(fs::read_to_string(&file).unwrap(), payload);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!f.vault.join(handle).exists());
    let found = f.local(json!({"op":"search","query":"shard replicated"}));
    assert!(
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["source_ref"] == handle)
    );
    f.index();
    let after = f.local(json!({"op":"search","query":"shard replicated"}));
    assert!(
        after["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["source_ref"] == handle)
    );
    let secret = format!("SUPABASE_SERVICE_ROLE_KEY=CANDIDATE{}", "X".repeat(24));
    let rejected = f.local(json!({"op":"sandbox","title":"secret","text":secret}));
    assert_eq!(rejected["ok"], false);
    assert!(!rejected.to_string().contains("CANDIDATE"));
    assert_eq!(fs::read_dir(f.home.join("sandbox")).unwrap().count(), 1);
    assert_eq!(f.ask(&json!({"op":"sandbox","text":"literature","provider":"google","base_url":"https://google.example"}))["ok"], false);
}

#[test]
fn safe_remote_proposals_stay_outside_vault_and_cannot_apply_by_default() {
    let f = Fixture::new();
    f.write(
        "projects/demo.md",
        "# Project Demo\n\n## Overview\nInitial content.\n\n## Log\n\n",
    );
    f.index();
    let before = fs::read(f.vault.join("projects/demo.md")).unwrap();
    let proposal = f.ask_with(&json!({"op":"propose","project":"demo","target":"projects/demo.md","section":"Log","text":"- Candidate pending human review.","provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert_eq!(proposal["ok"], true);
    assert!(proposal.get("diff").is_none());
    assert_eq!(fs::read(f.vault.join("projects/demo.md")).unwrap(), before);
    let proposal_id = proposal["proposal_id"].as_str().unwrap();
    let file = f.home.join("proposals").join(format!("{proposal_id}.json"));
    assert!(file.is_file());
    let record: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(record["classification"], "local_only");
    assert_eq!(record["status"], "proposed_review_required");
    let apply = f.ask_with(&json!({"op":"apply","proposal_id":proposal_id,"provider":"openai","base_url":"https://api.openai.com"}), &[("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS", "openai")]);
    assert!(apply.get("blocked").is_some());

    let secret = format!("SUPABASE_SERVICE_ROLE_KEY=CANDIDATE{}", "X".repeat(24));
    let secret_proposal = f.local(json!({"op":"propose","project":"demo","target":"projects/demo.md","section":"Log","text":secret}));
    assert_eq!(secret_proposal["ok"], false);
    assert!(!secret_proposal.to_string().contains("CANDIDATE"));
    let traversal = f.local(json!({"op":"propose","project":"demo","target":"STATUS.md","section":"Log","text":"Candidate."}));
    assert_eq!(traversal["ok"], false);
    assert_eq!(
        f.ask(&json!({"op":"proposal-list"}))["proposals"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn read_requires_index_checks_staleness_and_rejects_traversal_or_symlink_replacement() {
    let f = Fixture::new();
    f.write(
        "projects/atlas.md",
        "# Atlas\n\n## Decisions\nSQLite FTS5 is used for retrieval provenance.\n",
    );
    assert_eq!(
        f.local(json!({"op":"read","source_ref":"projects/atlas.md"}))["ok"],
        false
    );
    f.index();
    let read = f.local(json!({"op":"read","source_ref":"projects/atlas.md","heading":"Decisions"}));
    assert_eq!(read["ok"], true);
    assert!(read["content"].as_str().unwrap().contains("SQLite FTS5"));
    f.write("projects/atlas.md", "# Changed\n");
    assert_eq!(
        f.local(json!({"op":"read","source_ref":"projects/atlas.md"}))["stale"],
        true
    );
    assert_eq!(
        f.local(json!({"op":"read","source_ref":"../outside.md"}))["ok"],
        false
    );
    let outside = f.root.join("outside.md");
    fs::write(&outside, "# Outside\nprivate symlink target\n").unwrap();
    fs::remove_file(f.vault.join("projects/atlas.md")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, f.vault.join("projects/atlas.md")).unwrap();
    assert_eq!(
        f.local(json!({"op":"read","source_ref":"projects/atlas.md"}))["ok"],
        false
    );
}

#[test]
fn reindex_updates_only_derived_index_and_removes_deleted_notes() {
    let f = Fixture::new();
    f.write(
        "projects/atlas.md",
        "# Atlas\n\nCompaction retrieval indexing.\n",
    );
    let original = fs::read(f.vault.join("projects/atlas.md")).unwrap();
    let first = f.index();
    assert_eq!(first["ok"], true);
    let unchanged = f.ask(&json!({"op":"index"}));
    assert_eq!(unchanged["updated"], 0);
    assert_eq!(
        fs::read(f.vault.join("projects/atlas.md")).unwrap(),
        original
    );
    fs::remove_file(f.vault.join("projects/atlas.md")).unwrap();
    let removed = f.ask(&json!({"op":"index"}));
    assert_eq!(removed["removed_from_index"], 1);
    assert!(!f.vault.join("projects/atlas.md").exists());
}

#[test]
fn availability_reports_corruption_degrades_search_and_quarantines_before_rebuild() {
    let f = Fixture::new();
    for i in 0..6 {
        let body = if i < 4 {
            "Decision log migration marker."
        } else {
            "Unrelated retrieval content."
        };
        f.write(
            &format!("projects/note-{i}.md"),
            &format!("# Note {i}\n\n{body}\n"),
        );
    }
    f.index();
    let healthy = f.ask(&json!({"op":"repair"}));
    assert_eq!(healthy["repaired"], false);
    assert_eq!(healthy["integrity"], "ok");
    let path = f.home.join("index.sqlite3");
    let mut bytes = fs::read(&path).unwrap();
    bytes[24..104].fill(0xaa);
    fs::write(&path, bytes).unwrap();
    let status = f.ask(&json!({"op":"status"}));
    assert_eq!(status["integrity_ok"], false);
    assert_eq!(status["readable"], false);
    let degraded = f.local(json!({"op":"search","query":"decision log migration"}));
    assert_eq!(degraded["degraded"], true);
    assert!(!degraded["results"].as_array().unwrap().is_empty());
    let absent = f.local(json!({"op":"search","query":"the and of a"}));
    assert!(absent["results"].as_array().unwrap().is_empty());
    let repaired = f.ask(&json!({"op":"repair"}));
    assert_eq!(repaired["repaired"], true);
    assert!(!repaired["quarantined"].as_array().unwrap().is_empty());
    let status_after = f.ask(&json!({"op":"status"}));
    assert_eq!(status_after["integrity"], "ok");
    assert!(
        !f.local(json!({"op":"search","query":"decision log migration"}))["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn schema_v3_migration_preserves_sources_search_and_external_fts_edit_delete_sync() {
    let f = Fixture::new();
    f.write(
        "projects/decision-log.md",
        "# Decision Log\n\nMigration decision log marker.\n",
    );
    f.write(
        "projects/decision-log-archive.md",
        "# Decision Log Archive\n\nOlder decision log marker.\n",
    );
    f.write("projects/other.md", "# Other\n\nUnrelated material.\n");
    f.index();
    let db = f.home.join("index.sqlite3");
    let before_search = f.local(json!({"op":"search","query":"decision log"}));
    let before_refs: Vec<_> = before_search["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["source_ref"].as_str().unwrap().to_owned())
        .collect();
    assert!(!before_refs.is_empty());
    let con = Connection::open(&db).unwrap();
    let docs_before: i64 = con
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    let terms_before: i64 = con
        .query_row("SELECT COUNT(*) FROM term_df", [], |r| r.get(0))
        .unwrap();
    con.execute_batch("DROP TRIGGER IF EXISTS documents_fts_ai; DROP TRIGGER IF EXISTS documents_fts_ad; DROP TRIGGER IF EXISTS documents_fts_au; CREATE VIRTUAL TABLE documents_fts_v3 USING fts5(relpath UNINDEXED,title,headings,body,tokenize='porter unicode61'); INSERT INTO documents_fts_v3(rowid,relpath,title,headings,body) SELECT id,relpath,title,headings,body FROM documents; DROP TABLE documents_fts; ALTER TABLE documents_fts_v3 RENAME TO documents_fts; ALTER TABLE documents DROP COLUMN headings; CREATE TABLE doc_terms(relpath TEXT NOT NULL,term TEXT NOT NULL,PRIMARY KEY(relpath,term)) WITHOUT ROWID; CREATE INDEX doc_terms_by_term ON doc_terms(term); CREATE VIRTUAL TABLE temp._inst USING fts5vocab(main,documents_fts,'instance'); INSERT OR IGNORE INTO doc_terms SELECT f.relpath,v.term FROM temp._inst v JOIN documents_fts f ON f.rowid=v.doc; UPDATE metadata SET value='3' WHERE key='schema_version'; DELETE FROM metadata WHERE key='last_migration';").unwrap();
    drop(con);
    let migrated = f.ask(&json!({"op":"index"}));
    assert_eq!(migrated["ok"], true);
    let con = Connection::open(&db).unwrap();
    let docs_after: i64 = con
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    let terms_after: i64 = con
        .query_row("SELECT COUNT(*) FROM term_df", [], |r| r.get(0))
        .unwrap();
    let schema: String = con
        .query_row(
            "SELECT value FROM metadata WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let last_migration: Option<String> = con
        .query_row(
            "SELECT value FROM metadata WHERE key='last_migration'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let fts_sql: String = con
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='documents_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let headings: i64 = con
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE headings != ''",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(docs_after, docs_before);
    assert_eq!(terms_after, terms_before);
    assert_eq!(schema, "4");
    assert!(last_migration.is_some());
    assert!(fts_sql.contains("content='documents'"));
    assert_eq!(headings, docs_after);
    assert_eq!(
        con.query_row::<i64, _, _>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='doc_terms'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    drop(con);
    let after_search = f.local(json!({"op":"search","query":"decision log"}));
    let after_refs: Vec<_> = after_search["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["source_ref"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(before_refs, after_refs);

    f.write("projects/other.md", "# Other\n\neditprobeunique\n");
    f.ask(&json!({"op":"index"}));
    let found = f.local(json!({"op":"search","query":"editprobeunique"}));
    assert_eq!(found["shown"], 1);
    fs::remove_file(f.vault.join("projects/other.md")).unwrap();
    f.ask(&json!({"op":"index"}));
    let deleted = f.local(json!({"op":"search","query":"editprobeunique"}));
    assert_eq!(deleted["shown"], 0);
}

#[test]
fn recorded_schema_contract_matches_golden_without_size_overflow() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let schema = f.ask(&json!({"op":"schema"}));
    let golden: Value =
        serde_json::from_str(include_str!("../../bench/golden/format.json")).unwrap();

    for key in [
        "schema_version",
        "fts_tokenizer",
        "migration_policy",
        "rollback",
    ] {
        assert_eq!(schema[key], golden[key], "schema contract drifted at {key}");
    }
    for key in ["tables", "documents_columns"] {
        let actual: std::collections::HashSet<_> = schema[key]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect();
        let expected: std::collections::HashSet<_> = golden[key]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect();
        assert_eq!(actual, expected, "schema contract drifted at {key}");
    }
    let ceiling = golden["bytes_per_document_ceiling"].as_u64().unwrap();
    assert!(schema["bytes_per_document"].as_u64().unwrap() <= ceiling);
}

#[test]
fn mcp_protocol_keeps_tools_annotations_prompts_and_byte_stable_results() {
    let f = Fixture::new();
    probe_corpus(&f);
    f.index();
    let version = f.command(&["--version"]).output().unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    let cargo_version = env!("CARGO_PKG_VERSION");
    assert!(version.contains(cargo_version));
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"memory_fabric_system_prefix"}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_fabric_search","arguments":{"query":"SQLite FTS5 provenance"}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory_fabric_search","arguments":{"query":"SQLite FTS5 provenance"}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"prompts/list","params":{}}),
    ];
    let responses = f.ask_many(&["--mcp"], &requests, &[]);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "local-memory-fabric-mcp"
    );
    assert!(responses[0]["result"]["capabilities"]["prompts"].is_object());
    let names: Vec<_> = responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for name in [
        "memory_fabric_search",
        "memory_fabric_context",
        "memory_fabric_read",
        "memory_fabric_compact",
        "memory_fabric_federate",
        "memory_fabric_symbiosis",
        "memory_fabric_fuzzy_sections",
    ] {
        assert!(names.contains(&name), "MCP tool missing: {name}");
    }
    for tool in responses[1]["result"]["tools"].as_array().unwrap() {
        assert!(tool.get("annotations").is_some());
        assert_eq!(tool["annotations"]["destructiveHint"], false);
    }
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let compact = tools
        .iter()
        .find(|tool| tool["name"] == "memory_fabric_compact")
        .unwrap();
    assert_eq!(compact["annotations"]["readOnlyHint"], false);
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "memory_fabric_search")
        .unwrap();
    assert_eq!(search["annotations"]["readOnlyHint"], true);
    let prompt = responses[2]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(prompt.contains("Local Memory Fabric"));
    assert!(!prompt.contains("2026-"));
    assert_eq!(
        responses[3]["result"]["content"][0]["text"],
        responses[4]["result"]["content"][0]["text"]
    );
    let search: Value = serde_json::from_str(
        responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let uri = search["results"][0]["uri"].as_str().unwrap();
    assert!(uri.starts_with("file:///"));
    assert!(uri.ends_with("#L1"));
    assert_eq!(
        responses[5]["result"]["prompts"][0]["name"],
        "memory_fabric_system_prefix"
    );
}

#[test]
fn wal_reader_remains_available_while_a_writer_holds_the_index() {
    let f = Fixture::new();
    f.write(
        "projects/note.md",
        "# Decision Log\n\nMigration decision log marker.\n",
    );
    f.index();
    let conn = Connection::open(f.home.join("index.sqlite3")).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_ascii_lowercase(), "wal");
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let result = f.local(json!({"op":"search","query":"decision log"}));
    assert_eq!(result["ok"], true);
    assert!(!result["results"].as_array().unwrap().is_empty());
    conn.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn busy_writer_is_reported_as_transient_and_index_retry_succeeds() {
    let f = Fixture::new();
    f.write("projects/note.md", "# Note\n\nalpha beta gamma\n");
    f.index();
    let conn = Connection::open(f.home.join("index.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE")
        .unwrap();
    let busy = f.ask(&json!({"op":"index","force":true}));
    assert_eq!(busy["ok"], false);
    assert!(busy["error"].as_str().unwrap().contains("busy"));
    assert!(busy["error"].as_str().unwrap().contains("retry shortly"));
    assert!(busy["error"].as_str().unwrap().contains("locked"));
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(f.ask(&json!({"op":"index","force":true}))["ok"], true);
}

#[test]
fn staged_git_revert_point_restores_prewrite_state_and_negative_control_does_not() {
    fn scenario(root: &Path, stage_before_write: bool) -> String {
        let repo = root.join(if stage_before_write {
            "with-stage"
        } else {
            "without-stage"
        });
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "Engram Test"]);
        let file = repo.join("note.md");
        fs::write(&file, "V1 committed\n").unwrap();
        git(&["add", "note.md"]);
        git(&["commit", "-qm", "v1"]);
        fs::write(&file, "V2 the state about to be overwritten\n").unwrap();
        if stage_before_write {
            git(&["add", "--", "note.md"]);
        }
        fs::write(&file, "V3 clobbering write\n").unwrap();
        git(&["checkout-index", "-f", "--", "note.md"]);
        fs::read_to_string(file).unwrap()
    }
    let f = Fixture::new();
    let without = scenario(&f.root, false);
    let with = scenario(&f.root, true);
    assert!(
        with.contains("V2"),
        "pre-write content must be recoverable: {with}"
    );
    assert!(
        !without.contains("V2"),
        "without staging the revert point, the edit must be lost: {without}"
    );
    assert!(without.contains("V1"));
}
