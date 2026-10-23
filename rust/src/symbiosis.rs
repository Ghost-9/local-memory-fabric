use crate::MemoryFabric;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

pub const RERANK_CANDIDATE_LIMIT: usize = 256;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeAnchor {
    pub symbol: String,
    pub path: String,
    pub line: Option<usize>,
    pub proximity: f64,
}

impl MemoryFabric {
    /// Discovers code anchors from memory cards and cross-references with workspace / fsearch index.
    pub fn code_symbiosis(
        &self,
        query: &str,
        active_file: Option<&str>,
        repo_root: Option<&str>,
        limit: usize,
        provider: Option<&str>,
    ) -> Result<Value, String> {
        let base_search = self.search(
            query,
            "all",
            RERANK_CANDIDATE_LIMIT,
            provider,
            None,
            Some(true),
            None,
        )?;
        let results_arr = base_search["results"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut enriched_results = Vec::new();

        for item in results_arr.iter().take(RERANK_CANDIDATE_LIMIT) {
            let source_ref = item["source_ref"].as_str().unwrap_or("");
            let excerpt = item["excerpt"].as_str().unwrap_or("");
            let title = item["title"].as_str().unwrap_or("");

            // 1. Extract potential symbol references (`foo_bar`, [[file]], [file](path))
            let extracted_symbols = extract_symbols(excerpt);
            let mut anchors = Vec::new();

            for sym in &extracted_symbols {
                // If it looks like a path or symbol, check proximity to active_file
                let (resolved_path, resolved_line) =
                    resolve_with_fsearch(sym, repo_root).unwrap_or_else(|| (sym.clone(), None));

                let prox = calculate_path_proximity(active_file, &resolved_path);
                anchors.push(CodeAnchor {
                    symbol: sym.clone(),
                    path: resolved_path,
                    line: resolved_line,
                    proximity: prox,
                });
            }

            // Max proximity from any anchor or source ref
            let card_prox = anchors.iter().map(|a| a.proximity).fold(
                calculate_path_proximity(active_file, source_ref),
                |max_p, p| max_p.max(p),
            );

            let base_score = item["score"].as_f64().unwrap_or(0.0);
            let fused_score = (base_score * 0.65) + (card_prox * 0.35);

            enriched_results.push(json!({
                "source_ref": source_ref,
                "title": title,
                "excerpt": excerpt,
                "base_score": base_score,
                "proximity": card_prox,
                "fused_score": fused_score,
                "code_anchors": anchors,
                "section_anchor": item["section_anchor"],
                "section_breadcrumb": item["section_breadcrumb"],
            }));
        }

        // Rerank top 256 candidates by fused_score descending
        enriched_results.sort_by(|a, b| {
            let sa = a["fused_score"].as_f64().unwrap_or(0.0);
            let sb = b["fused_score"].as_f64().unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

        let final_count = enriched_results.len().min(limit.clamp(1, 50));
        enriched_results.truncate(final_count);

        Ok(json!({
            "ok": true,
            "query": query,
            "active_file": active_file,
            "candidates_reranked": RERANK_CANDIDATE_LIMIT.min(results_arr.len()),
            "shown": enriched_results.len(),
            "results": enriched_results,
            "engine": "fsearch_memory_symbiosis_v1.9"
        }))
    }
}

/// Extracts symbols from text (`symbol`, [[symbol]], or [text](path#Lline))
pub fn extract_symbols(text: &str) -> Vec<String> {
    let mut symbols = HashSet::new();

    // Markdown links [text](path)
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find(']') {
            let label = &rest[..close];
            if rest[close..].starts_with("](") {
                let link_part = &rest[close + 2..];
                if let Some(paren_close) = link_part.find(')') {
                    let target = &link_part[..paren_close];
                    if !target.is_empty() {
                        symbols.insert(target.split('#').next().unwrap_or(target).to_string());
                    }
                }
            } else if rest[close..].starts_with("]]") {
                symbols.insert(label.to_string());
            }
        }
    }

    // Backtick symbols `foo_bar`
    let mut b_rest = text;
    while let Some(start) = b_rest.find('`') {
        b_rest = &b_rest[start + 1..];
        if let Some(end) = b_rest.find('`') {
            let sym = &b_rest[..end].trim();
            if sym.len() >= 3
                && !sym.contains(' ')
                && (sym.contains('_')
                    || sym.contains('.')
                    || sym.contains('/')
                    || sym.contains("::"))
            {
                symbols.insert(sym.to_string());
            }
            b_rest = &b_rest[end + 1..];
        } else {
            break;
        }
    }

    symbols.into_iter().collect()
}

/// Calculates path distance / proximity between active file and candidate path in [0.0, 1.0].
pub fn calculate_path_proximity(active: Option<&str>, candidate: &str) -> f64 {
    let act = match active {
        Some(a) if !a.trim().is_empty() => a.trim(),
        _ => return 0.1,
    };

    if act == candidate {
        return 1.0;
    }

    let p_act = Path::new(act);
    let p_cand = Path::new(candidate);

    let act_comps: Vec<_> = p_act.components().collect();
    let cand_comps: Vec<_> = p_cand.components().collect();

    let mut common = 0;
    for (a, c) in act_comps.iter().zip(cand_comps.iter()) {
        if a == c {
            common += 1;
        } else {
            break;
        }
    }

    let max_len = act_comps.len().max(cand_comps.len());
    if max_len == 0 {
        return 0.1;
    }

    // Ratio of shared path prefix
    let ratio = (common as f64) / (max_len as f64);
    (ratio * 0.8 + 0.1).clamp(0.1, 1.0)
}

/// Invokes fsearch daemon CLI if present on system to resolve symbol to exact file and line.
pub fn resolve_with_fsearch(
    symbol: &str,
    repo_root: Option<&str>,
) -> Option<(String, Option<usize>)> {
    if symbol.trim().is_empty() {
        return None;
    }

    // 1. Try fsearch CLI at ~/.local/bin/fsearch or in PATH
    let fsearch_paths = ["/Users/batra/.local/bin/fsearch", "fsearch"];

    for bin in &fsearch_paths {
        let mut cmd = Command::new(bin);
        cmd.arg(symbol).arg("--json");
        if let Some(root) = repo_root {
            cmd.current_dir(root);
        }

        if let Ok(output) = cmd.output()
            && output.status.success()
            && let Ok(val) = serde_json::from_slice::<Value>(&output.stdout)
            && let Some(first_match) = val.as_array().and_then(|arr| arr.first())
        {
            let path = first_match["path"]
                .as_str()
                .or_else(|| first_match.as_str());
            if let Some(p) = path {
                let line = first_match["line"].as_u64().map(|l| l as usize);
                return Some((p.to_string(), line));
            }
        }
    }

    None
}
