use crate::MemoryFabric;
use crate::db::map_candidate;
use crate::paths::{parse_optional_date, safe_relpath};
use crate::text::{json_error, regex, sha256, tokenize};
use chrono::NaiveDate;
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;

impl MemoryFabric {
    pub(crate) fn search(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
        provider: Option<&str>,
        as_of: Option<&str>,
        loopback: Option<bool>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        if query.trim().is_empty() {
            return Ok(json_error("query is required"));
        }
        let as_of_date = match as_of {
            Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                Ok(d) => Some(d),
                Err(_) => return Ok(json_error("as_of must be an ISO date (YYYY-MM-DD)")),
            },
            None => None,
        };
        let terms = tokenize(query);
        if terms.is_empty() {
            return Ok(json!({"ok":true,"results":[],"abstention":"no searchable terms"}));
        }
        let con = self.connect()?;
        let n_docs: i64 = con
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        // Fetch document frequencies for this query's own tokens only. Reading the whole term_df
        // table was the only super-linear step in the hot path. A token with no row keeps df 0 and
        // is dropped by the gate below, exactly as before.
        let probe: Vec<&str> = terms.iter().take(64).map(|t| t.as_str()).collect();
        let mut df: HashMap<String, i64> = HashMap::new();
        if !probe.is_empty() {
            let placeholders = (1..=probe.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT term,df FROM term_df WHERE term IN ({placeholders})");
            let mut stmt = con.prepare(&sql).map_err(|e| e.to_string())?;
            let params: Vec<&dyn rusqlite::ToSql> =
                probe.iter().map(|t| t as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (t, n) = row.map_err(|e| e.to_string())?;
                df.insert(t, n);
            }
        }
        let max_df = std::cmp::max(2, (n_docs as f64 * 0.25) as i64);
        let rank_key = |a: &String, b: &String| {
            df[a]
                .cmp(&df[b])
                .then_with(|| b.len().cmp(&a.len()))
                .then_with(|| a.cmp(b))
        };
        let indexed: Vec<String> = terms
            .iter()
            .filter(|t| df.get(*t).copied().unwrap_or(0) > 0)
            .cloned()
            .collect();
        let mut useful: Vec<String> = indexed
            .iter()
            .filter(|t| df.get(*t).copied().unwrap_or(0) <= max_df)
            .cloned()
            .collect();
        useful.sort_by(|a, b| rank_key(a, b));
        useful.truncate(16);
        // Every query token is indexed but all are too common to discriminate. Returning silence
        // here reads to a caller as "the vault has nothing on this", which is false. Answer over
        // the query's own tokens and label the result; abstention is reserved for tokens the index
        // does not contain at all.
        let mut low_discrimination = false;
        if useful.is_empty() {
            if indexed.is_empty() {
                return Ok(json!({
                    "ok": true,
                    "results": [],
                    "shown": 0,
                    "total_matches": 0,
                    "low_discrimination": false,
                    "abstention": "no indexed source contains these terms",
                }));
            }
            low_discrimination = true;
            let mut fallback = indexed;
            fallback.sort_by(|a, b| rank_key(a, b));
            fallback.truncate(16);
            useful = fallback;
        }
        let match_query = useful
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        // Total matches for the same predicate, so a caller can tell "no match" from "no data"
        // and can see when the overlap or temporal gate withheld what did match.
        let total_matches: i64 = con
            .query_row(
                "SELECT COUNT(*) FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath WHERE documents_fts MATCH ?1 AND (?2='all' OR ?2='' OR d.scope=?2)",
                params![&match_query, &scope],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut stmt=con.prepare("SELECT d.relpath,d.title,d.scope,d.classification,d.source_status,d.valid_from,d.valid_to,d.superseded_by,d.provider_policy,d.export_policy,d.body,d.indexed_at,bm25(documents_fts,0.0,4.0,1.5,1.0) AS rank FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath WHERE documents_fts MATCH ?1 AND (?2='all' OR ?2='' OR d.scope=?2) ORDER BY rank, documents_fts.relpath LIMIT ?3").map_err(|e|e.to_string())?;
        let raw = stmt
            .query_map(
                params![match_query, scope, (limit.clamp(1, 20) * 10) as i64],
                map_candidate,
            )
            .map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
        for row in raw {
            candidates.push(row.map_err(|e| e.to_string())?);
        }
        // One lookup for every candidate instead of one query per candidate.
        let mut terms_by_relpath: HashMap<String, HashSet<String>> = HashMap::new();
        if !candidates.is_empty() {
            let relpaths: Vec<&str> = candidates.iter().map(|c| c.relpath.as_str()).collect();
            let placeholders = (1..=relpaths.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT relpath,term FROM doc_terms WHERE relpath IN ({placeholders})");
            let mut q = con.prepare(&sql).map_err(|e| e.to_string())?;
            let params: Vec<&dyn rusqlite::ToSql> =
                relpaths.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let iter = q
                .query_map(params.as_slice(), |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            for x in iter {
                let (rel, term) = x.map_err(|e| e.to_string())?;
                terms_by_relpath.entry(rel).or_default().insert(term);
            }
        }
        let mut results = Vec::new();
        let mut excluded_temporal = 0;
        let mut temporal_without_as_of = false;
        // Per-note export policy. Off by default so the operator's blanket read exemption keeps
        // working; when on, a note must be marked allow_remote before a non-loopback caller may
        // see it, and the count is reported.
        let mut withheld = 0u64;
        if self.export_enforced() && loopback != Some(true) {
            withheld = candidates
                .iter()
                .filter(|c| c.export_policy != "allow_remote")
                .count() as u64;
            candidates.retain(|c| c.export_policy == "allow_remote");
        }
        for row in candidates {
            let valid_from = match parse_optional_date(row.valid_from.as_deref()) {
                Ok(x) => x,
                Err(()) => {
                    excluded_temporal += 1;
                    continue;
                }
            };
            let valid_to = match parse_optional_date(row.valid_to.as_deref()) {
                Ok(x) => x,
                Err(()) => {
                    excluded_temporal += 1;
                    continue;
                }
            };
            let status = row.source_status.trim().to_ascii_lowercase();
            if as_of_date.is_none() && (valid_from.is_some() || valid_to.is_some()) {
                temporal_without_as_of = true;
            }
            if let Some(d) = as_of_date {
                if valid_from.map(|v| d < v).unwrap_or(false)
                    || valid_to.map(|v| d > v).unwrap_or(false)
                {
                    continue;
                }
            }
            if ["superseded", "archived", "deprecated", "historical"].contains(&status.as_str())
                && (as_of_date.is_none()
                    || valid_to.is_none()
                    || as_of_date.unwrap() > valid_to.unwrap())
            {
                continue;
            }
            let doc_terms = terms_by_relpath.get(&row.relpath).cloned().unwrap_or_default();
            let matched: Vec<String> = useful
                .iter()
                .filter(|t| doc_terms.contains(t.as_str()))
                .cloned()
                .collect();
            let distinctive = matched
                .iter()
                .any(|t| t.len() >= 9 && df.get(t).copied() == Some(1));
            if matched.len() < 2 && !distinctive {
                continue;
            }
            let folded = row.body.to_lowercase();
            let byte_start = matched
                .iter()
                .filter_map(|t| folded.find(t))
                .min()
                .unwrap_or(0);
            let start = row
                .body
                .get(..byte_start)
                .map(|prefix| prefix.chars().count())
                .unwrap_or(0)
                .saturating_sub(240);
            let body_chars = row.body.chars().count();
            let mut excerpt = row
                .body
                .chars()
                .skip(start)
                .take(1000)
                .collect::<String>()
                .trim()
                .to_string();
            if start > 0 {
                excerpt = format!("…{excerpt}");
            }
            if start + 1000 < body_chars {
                excerpt.push('…');
            }
            results.push(json!({"source_ref":row.relpath,"title":row.title,"scope":row.scope,"classification":row.classification,"source_status":row.source_status,"valid_from":row.valid_from,"valid_to":row.valid_to,"superseded_by":row.superseded_by,"provider_policy":row.provider_policy,"export_policy":row.export_policy,"excerpt":excerpt,"matched_terms":matched,"bm25_rank":(row.rank*1_000_000.0).round()/1_000_000.0,"indexed_at":row.indexed_at}));
            if results.len() >= limit.clamp(1, 20) {
                break;
            }
        }
        let mut warning: Option<String> = None;
        'outer: for i in 0..results.len() {
            for j in i + 1..results.len() {
                let a = &results[i];
                let b = &results[j];
                if a["scope"] != b["scope"] || a["source_status"] == b["source_status"] {
                    continue;
                }
                let a0 = a["valid_from"].as_str().unwrap_or("0001-01-01");
                let a1 = a["valid_to"].as_str().unwrap_or("9999-12-31");
                let b0 = b["valid_from"].as_str().unwrap_or("0001-01-01");
                let b1 = b["valid_to"].as_str().unwrap_or("9999-12-31");
                if a0 <= b1 && b0 <= a1 {
                    warning=Some("Overlapping source lifecycle records; inspect provenance and do not auto-resolve the conflict.".into());
                    break 'outer;
                }
            }
        }
        if excluded_temporal > 0 {
            warning =
                Some("Some candidates had invalid temporal metadata and were excluded.".into());
        } else if temporal_without_as_of && as_of.is_none() {
            warning=Some("No as_of date supplied; validity bounds were not evaluated against the host clock.".into());
        }
        if low_discrimination && warning.is_none() {
            warning = Some("Query terms were too common in this index to discriminate; these are unfiltered candidates, not a ranked answer.".into());
        }
        let shown = results.len();
        let abstention = if shown > 0 {
            None
        } else if total_matches == 0 {
            Some("no indexed source matches these terms")
        } else {
            Some("matches exist but none passed the overlap or temporal gate; inspect total_matches and the date")
        };
        Ok(
            json!({"ok":true,"results":results,"shown":shown,"total_matches":total_matches,"low_discrimination":low_discrimination,"abstention":abstention,"temporal_warning":warning,"as_of":as_of,"withheld_export":withheld,"local_only":true,"note":"Lexical candidates are not verified facts; inspect cited source passages."}),
        )
    }

    pub(crate) fn context(
        &self,
        project: Option<&str>,
        query: &str,
        provider: Option<&str>,
        budget: usize,
        as_of: Option<&str>,
        loopback: Option<bool>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        let scope = project
            .map(|p| format!("project:{p}"))
            .unwrap_or_else(|| "all".into());
        let hits = self.search(query, &scope, 10, provider, as_of, loopback)?;
        if !hits["ok"].as_bool().unwrap_or(false) {
            return Ok(hits);
        }
        let char_budget = budget.clamp(100, 6000) * 4;
        let mut sections = Vec::new();
        let mut used = 0usize;
        if let Some(items) = hits["results"].as_array() {
            for item in items {
                let temporal = format!(
                    "; valid {} to {}",
                    item["valid_from"].as_str().unwrap_or("unspecified"),
                    item["valid_to"].as_str().unwrap_or("open")
                );
                let block = format!(
                    "### {} — {}\nstatus={}; class={}{}; provider={}; export={}\n{}\n",
                    item["title"].as_str().unwrap_or(""),
                    item["source_ref"].as_str().unwrap_or(""),
                    item["source_status"].as_str().unwrap_or("unspecified"),
                    item["classification"].as_str().unwrap_or("local_only"),
                    temporal,
                    item["provider_policy"]
                        .as_str()
                        .unwrap_or("loopback_allowlist_only"),
                    item["export_policy"].as_str().unwrap_or("deny_remote"),
                    item["excerpt"].as_str().unwrap_or("")
                );
                if used + block.chars().count() > char_budget {
                    let remain = char_budget.saturating_sub(used);
                    if remain > 100 {
                        sections.push(block.chars().take(remain).collect::<String>());
                        used += remain;
                    }
                    break;
                }
                used += block.chars().count();
                sections.push(block);
            }
        }
        Ok(
            json!({"ok":true,"project":project,"source_count":sections.len(),"estimated_characters":used,"char_budget":char_budget,"context":sections.join(""),"abstention":hits["abstention"],"as_of":hits["as_of"],"temporal_warning":hits["temporal_warning"],"instruction":"Treat excerpts as source evidence, not verified conclusions; cite source_ref and distinguish inference."}),
        )
    }

    pub(crate) fn read(
        &self,
        source_ref: &str,
        provider: Option<&str>,
        heading: Option<&str>,
        loopback: Option<bool>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        let con = self.connect()?;
        let row = match self.indexed_doc(source_ref, &con) {
            Ok(v) => v,
            Err(e) => return Ok(json_error(e)),
        };
        // Same per-note rule as search: without it, `read` would be a hole straight through the
        // gate that search enforces.
        if self.export_enforced()
            && loopback != Some(true)
            && row["export_policy"].as_str().unwrap_or("deny_remote") != "allow_remote"
        {
            return Ok(
                json!({"ok":false,"blocked":"This source is not approved for export to a non-local model provider; nothing was returned."}),
            );
        }
        let body = row["body"].as_str().unwrap_or("");
        let current = fs::read_to_string(self.vault.join(safe_relpath(source_ref)?));
        if current
            .as_ref()
            .map(|s| sha256(s) != row["content_hash"].as_str().unwrap_or(""))
            .unwrap_or(true)
        {
            return Ok(
                json!({"ok":false,"stale":true,"message":"Source changed or disappeared; run explicit reindex before reading."}),
            );
        }
        let mut output = body.to_string();
        if let Some(wanted) = heading {
            let lines: Vec<&str> = body.lines().collect();
            let start = lines.iter().position(|line| {
                regex(&format!(
                    r"(?i)^#{{1,6}}\s+{}\s*#*\s*$",
                    regex::escape(wanted)
                ))
                .is_match(line.trim())
            });
            let Some(start) = start else {
                return Ok(json_error("requested heading was not found"));
            };
            let level = lines[start].chars().take_while(|c| *c == '#').count();
            let mut end = lines.len();
            for (i, line) in lines.iter().enumerate().skip(start + 1) {
                let n = line.chars().take_while(|c| *c == '#').count();
                if n > 0
                    && n <= level
                    && line
                        .chars()
                        .nth(n)
                        .map(char::is_whitespace)
                        .unwrap_or(false)
                {
                    end = i;
                    break;
                }
            }
            output = lines[start..end].join("\n");
        }
        let cap = self.config.max_result_chars;
        let truncated = output.chars().count() > cap;
        output = output.chars().take(cap).collect();
        Ok(
            json!({"ok":true,"source_ref":row["relpath"],"title":row["title"],"scope":row["scope"],"classification":row["classification"],"source_status":row["source_status"],"valid_from":row["valid_from"],"valid_to":row["valid_to"],"superseded_by":row["superseded_by"],"provider_policy":row["provider_policy"],"export_policy":row["export_policy"],"content":output,"truncated":truncated,"local_only":true}),
        )
    }
}
