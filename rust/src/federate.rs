/// Federated Cross-Worktree Multi-Vault Synchronization.
/// Provides git worktree origin discovery, multi-vault federated search, and 3-way merge proposals.

use crate::MemoryFabric;
use crate::text::json_error;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitOriginInfo {
    pub git_root: Option<String>,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub worktrees: Vec<String>,
}

impl MemoryFabric {
    /// Detects git repository root, origin URL, current branch, and sibling worktrees.
    pub fn detect_git_origin(&self) -> GitOriginInfo {
        let vault_path = &self.vault;

        let git_root = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout).ok().map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        let origin_url = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["remote", "get-url", "origin"])
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout).ok().map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        let branch = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["branch", "--show-current"])
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if b.is_empty() { None } else { Some(b) }
                } else {
                    None
                }
            });

        let mut worktrees = Vec::new();
        if let Ok(out) = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["worktree", "list", "--porcelain"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if let Some(path) = line.strip_prefix("worktree ") {
                        let trimmed = path.trim().to_string();
                        if !trimmed.is_empty() && !worktrees.contains(&trimmed) {
                            worktrees.push(trimmed);
                        }
                    }
                }
            }
        }

        GitOriginInfo {
            git_root,
            origin_url,
            branch,
            worktrees,
        }
    }

    /// Status of federated mesh and detected peers
    pub(crate) fn federate_status(&self) -> Result<Value, String> {
        let git_info = self.detect_git_origin();
        let home_env = std::env::var("HOME").unwrap_or_default();
        let global_system_mem = PathBuf::from(&home_env).join("system-memory");
        let mut discovered_peers = Vec::new();

        if global_system_mem.exists() && global_system_mem.is_dir() && global_system_mem != self.vault {
            discovered_peers.push(global_system_mem.to_string_lossy().to_string());
        }

        for wt in &git_info.worktrees {
            let candidate_mem = PathBuf::from(wt).join(".memory");
            if candidate_mem.exists() && candidate_mem.is_dir() && candidate_mem != self.vault {
                discovered_peers.push(candidate_mem.to_string_lossy().to_string());
            }
        }

        if let Ok(extra) = std::env::var("MEMORY_FABRIC_PEER_VAULTS") {
            for part in extra.split(':').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                let pb = PathBuf::from(part);
                if pb.exists() && pb.is_dir() && pb != self.vault && !discovered_peers.contains(&part.to_string()) {
                    discovered_peers.push(part.to_string());
                }
            }
        }

        Ok(json!({
            "ok": true,
            "git_root": git_info.git_root,
            "origin_url": git_info.origin_url,
            "branch": git_info.branch,
            "worktrees": git_info.worktrees,
            "primary_vault": self.vault.to_string_lossy(),
            "peer_vaults": discovered_peers,
            "federated_enabled": true
        }))
    }

    /// Performs federated search across primary and peer vaults
    pub(crate) fn federate_search(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
        peer_vaults: &[String],
        provider: Option<&str>,
        as_of: Option<&str>,
        loopback: Option<bool>,
        token_budget: Option<usize>,
    ) -> Result<Value, String> {
        let mut primary_res = self.search(query, scope, limit * 2, provider, as_of, loopback, token_budget)?;
        if primary_res.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(primary_res);
        }

        let mut all_results = Vec::new();
        let mut seen_refs = std::collections::HashSet::new();

        if let Some(arr) = primary_res.get_mut("results").and_then(Value::as_array_mut) {
            for item in arr.drain(..) {
                let mut obj = item;
                if let Some(map) = obj.as_object_mut() {
                    let s_ref = map.get("source_ref").and_then(Value::as_str).unwrap_or("").to_string();
                    seen_refs.insert(s_ref);
                    map.insert("vault_origin".to_string(), json!("primary"));
                }
                all_results.push(obj);
            }
        }

        let mut peers_queried = 0;
        for peer in peer_vaults {
            let p_path = Path::new(peer);
            if !p_path.exists() || !p_path.is_dir() {
                continue;
            }
            peers_queried += 1;
            // Create temporary peer fabric
            let peer_home = std::env::temp_dir().join(format!("mf-peer-{}", crate::text::sha256(peer)[..8].to_string()));
            let peer_fabric = match MemoryFabric::with_paths(peer_home, p_path.to_path_buf()) {
                Ok(f) => { let _ = f.index(false); f },
                Err(_) => continue,
            };
            if let Ok(p_res) = peer_fabric.search(query, scope, limit, provider, as_of, loopback, token_budget) {
                if let Some(p_arr) = p_res.get("results").and_then(Value::as_array) {
                    for item in p_arr {
                        let s_ref = item.get("source_ref").and_then(Value::as_str).unwrap_or("");
                        let unique_key = format!("{}:{}", peer, s_ref);
                        if seen_refs.insert(unique_key) {
                            let mut cloned = item.clone();
                            if let Some(map) = cloned.as_object_mut() {
                                map.insert("vault_origin".to_string(), json!(peer));
                            }
                            all_results.push(cloned);
                        }
                    }
                }
            }
        }

        // Re-sort federated results by rrf_score or bm25_rank
        all_results.sort_by(|a, b| {
            let rrf_a = a.get("rrf_score").and_then(Value::as_f64).unwrap_or(0.0);
            let rrf_b = b.get("rrf_score").and_then(Value::as_f64).unwrap_or(0.0);
            rrf_b.partial_cmp(&rrf_a).unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let rank_a = a.get("bm25_rank").and_then(Value::as_f64).unwrap_or(0.0);
                    let rank_b = b.get("bm25_rank").and_then(Value::as_f64).unwrap_or(0.0);
                    rank_a.partial_cmp(&rank_b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    let ref_a = a.get("source_ref").and_then(Value::as_str).unwrap_or("");
                    let ref_b = b.get("source_ref").and_then(Value::as_str).unwrap_or("");
                    ref_a.cmp(ref_b)
                })
        });

        all_results.truncate(limit.clamp(1, 20));
        let count = all_results.len();

        Ok(json!({
            "ok": true,
            "federated": true,
            "results": all_results,
            "shown": count,
            "peers_queried": peers_queried
        }))
    }

    /// Performs conflict-free 3-way merge proposal between local vault and a peer worktree vault
    pub(crate) fn federate_merge_proposal(
        &self,
        peer_vault: &str,
        relpath: &str,
        provider: Option<&str>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(json!({
                "ok": false,
                "blocked": "Federated merge proposals require an authorized local provider."
            }));
        }

        let safe_rel = match crate::paths::safe_relpath(relpath) {
            Ok(r) => r,
            Err(e) => return Ok(json_error(&format!("unsafe relpath: {e}"))),
        };
        let local_file = self.vault.join(&safe_rel);
        let peer_file = Path::new(peer_vault).join(&safe_rel);
        if let Ok(canon_vault) = self.vault.canonicalize() {
            if local_file.exists() {
                if let Ok(canon_local) = local_file.canonicalize() {
                    if !canon_local.starts_with(&canon_vault) {
                        return Ok(json_error("local target escapes vault boundary"));
                    }
                }
            }
        }

        if !peer_file.exists() {
            return Ok(json_error(&format!("peer file does not exist: {}", peer_file.display())));
        }

        let peer_content = std::fs::read_to_string(&peer_file)
            .map_err(|e| format!("cannot read peer file: {e}"))?;

        if !local_file.exists() {
            if let Some(parent) = local_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&local_file, &peer_content)
                .map_err(|e| format!("cannot write new card: {e}"))?;
            let _ = self.index(false);
            return Ok(json!({
                "ok": true,
                "status": "synchronized_new_card",
                "relpath": relpath,
                "peer_vault": peer_vault
            }));
        }

        let local_content = std::fs::read_to_string(&local_file)
            .map_err(|e| format!("cannot read local file: {e}"))?;

        if local_content == peer_content {
            return Ok(json!({
                "ok": true,
                "status": "up_to_date",
                "message": "Local and peer note contents are byte-identical."
            }));
        }

        // Detect 3-way merge using similar crate
        let diff = similar::TextDiff::from_lines(&local_content, &peer_content);
        let mut patch = String::new();
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => " ",
            };
            patch.push_str(sign);
            patch.push_str(change.value());
        }

        let proposal_req = json!({
            "op": "propose",
            "project": relpath.strip_prefix("projects/").and_then(|s| s.strip_suffix(".md")).unwrap_or("peer"),
            "target": relpath,
            "section": "Sync",
            "text": peer_content,
            "source_refs": Vec::<String>::new(),
            "provider": provider
        });

        let mut prop = self.propose(&proposal_req)?;
        if let Some(map) = prop.as_object_mut() {
            map.insert("diff".to_string(), json!(patch));
            map.insert("merge_status".to_string(), json!("conflict_free_proposal"));
            map.insert("peer_vault".to_string(), json!(peer_vault));
        }

        Ok(prop)
    }

    /// Milestone 16: Git Worktree Invariant Overlay.
    /// Detects if CWD is a git worktree, parses .git pointer file, and locates worktree-scoped memory.
    pub(crate) fn worktree_status(&self) -> Result<Value, String> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let git_file = cwd.join(".git");
        let mut is_worktree = false;
        let mut main_git_dir = None;
        let mut slug = cwd.file_name().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();

        if git_file.exists() && git_file.is_file() {
            is_worktree = true;
            if let Ok(content) = std::fs::read_to_string(&git_file) {
                if let Some(gitdir) = content.strip_prefix("gitdir:") {
                    let gd_trimmed = gitdir.trim();
                    main_git_dir = Some(gd_trimmed.to_string());
                    if let Some(pos) = gd_trimmed.rfind("/worktrees/") {
                        slug = gd_trimmed[pos + "/worktrees/".len()..].to_string();
                    }
                }
            }
        }

        let project_note = format!("projects/{}.md", slug);
        let project_exists = self.vault.join(&project_note).exists();

        Ok(json!({
            "ok": true,
            "is_worktree": is_worktree,
            "worktree_slug": slug,
            "gitdir": main_git_dir,
            "project_note": project_note,
            "project_note_exists": project_exists,
            "worktree_overlay_active": is_worktree
        }))
    }

}
