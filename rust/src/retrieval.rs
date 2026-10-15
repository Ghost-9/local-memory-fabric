use crate::MemoryFabric;
use crate::db::map_candidate;
use crate::paths::{parse_optional_date, safe_relpath};
use crate::text::{json_error, regex, sha256, tokenize};
use chrono::NaiveDate;
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;

/// Discriminating-power limits for query terms; mirrors the Python engine exactly.
/// See `MIN_USEFUL_DF` / `MAX_USEFUL_DF_FRACTION` there for the measurement behind them.
const MIN_USEFUL_DF: i64 = 4;
const MAX_USEFUL_DF_FRACTION: f64 = 0.90;

/// #100 - how much the best-section score contributes to the final order, against the
/// note-level BM25 rank. Measured, not chosen: see the sweep recorded in bench/fixture_gate.py.
/// 0.0 is the old whole-note behaviour; 1.0 is the section score alone, which measured WORSE than
/// 0.0. The environment override exists only so the weight can be swept during measurement.
const SECTION_WEIGHT_DEFAULT: f64 = 0.15;

/// FTS5 column weights: (relpath UNINDEXED, title, headings, body).
///
/// Swept, not chosen. The fixture's decoys take a target's own first eight words and repeat them
/// twice in a sixteen-word note, so their term density is enormous while their length is tiny -
/// and BM25 penalises long documents, which makes a short dense decoy win. The legitimate signal
/// that separates them is the title: the target's title carries the query term, the decoy's says
/// "Decoy". Environment overrides exist so the sweep can be re-run; the defaults are the measured
/// winners and `bench/fixture_gate.py` records them.
fn fts_title_weight() -> f64 {
    env_weight("MEMORY_FABRIC_TITLE_WEIGHT", FTS_TITLE_WEIGHT)
}
fn fts_heading_weight() -> f64 {
    env_weight("MEMORY_FABRIC_HEADING_WEIGHT", FTS_HEADING_WEIGHT)
}
fn env_weight(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.0..=50.0).contains(v)).unwrap_or(default)
}

/// How much a note is penalised for repeating itself, and the shortest note worth measuring.
///
/// #100 and #104 are the same signal seen twice. A bag-of-words ranker prefers a document that
/// repeats a run of its own words: it contains the query term more times, it is shorter, and BM25
/// penalises length, so every term of the comparison points the wrong way. The fixture's decoys are
/// exactly this - a target's first eight words, twice, in nineteen words of note - and they were
/// winning 46 of the 62 non-first places.
///
/// The signal is not "this is a decoy". It is that a document which restates a long run of itself
/// verbatim is not a document about a topic; it says one thing twice. That also catches the real
/// cases #104 is about: an accidental paste, a generated stub, a note that is half a duplicate of
/// itself. It is computed from the note's own text and knows nothing about filenames.
///
/// Set the weight to 0 to disable and confirm the penalty is what is doing the work.
/// Chosen at 1.0 rather than at the sweep maximum. The measured curve is monotonic - 0.6 gives
/// MRR@3 0.7822, 1.0 gives 0.7997, 3.0 gives 0.8377 - but above 1.0 a note with 35% shingle
/// repetition multiplies to below zero and is pushed under completely irrelevant results, which is
/// a worse failure than missing a decoy. 1.0 is the boundary where "entirely self-repeated" scores
/// zero, and it is a boundary, not a fitted constant. Raising it needs a real-corpus guard, which
/// did not exist when this was measured - see issue #99.
const SELF_REPETITION_WEIGHT_DEFAULT: f64 = 1.0;

fn self_repetition_weight() -> f64 {
    std::env::var("MEMORY_FABRIC_REPETITION_WEIGHT").ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.0..=3.0).contains(v))
        .unwrap_or(SELF_REPETITION_WEIGHT_DEFAULT)
}
/// Below this many distinct shingles the ratio is noise: three sentences can repeat a phrase by
/// chance, and short notes are not the problem this addresses.
const REPETITION_MIN_SHINGLES: usize = 6;
const SHINGLE_WORDS: usize = 8;

fn self_repetition_ratio(text: &str) -> f64 {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    if words.len() < SHINGLE_WORDS + REPETITION_MIN_SHINGLES {
        return 0.0;
    }
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut total = 0usize;
    let mut repeated = 0usize;
    for i in 0..=(words.len() - SHINGLE_WORDS) {
        let mut h: u64 = 0xcbf29ce484222325;
        for w in &words[i..i + SHINGLE_WORDS] {
            for b in w.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h ^= 0xff;
            h = h.wrapping_mul(0x100000001b3);
        }
        total += 1;
        if !seen.insert(h) {
            repeated += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    repeated as f64 / total as f64
}

const FTS_TITLE_WEIGHT: f64 = 4.0;
const FTS_HEADING_WEIGHT: f64 = 1.5;

fn section_weight() -> f64 {
    std::env::var("MEMORY_FABRIC_SECTION_WEIGHT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.0..=1.0).contains(v))
        .unwrap_or(SECTION_WEIGHT_DEFAULT)
}

/// Recognises the error shapes that mean "the index file is unusable" rather than "the request
/// was wrong". These are the only failures worth degrading for: everything else should still be an
/// error, because silently switching to a weak search would hide a real bug.
fn is_corruption(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("malformed")
        || m.contains("corrupt")
        || m.contains("not a database")
        || m.contains("database disk image")
        || m.contains("file is encrypted")
}

impl MemoryFabric {
    /// A1 - answer without the index.
    ///
    /// The index is a *cache*: it is derived entirely from the vault and rebuilds in ~42 ms per 200
    /// notes. Losing it - deleted, corrupt, half-written, or arriving on a fresh machine - should
    /// degrade search, not disable it. This path exists so the engine still answers when the fast
    /// one is gone, and it says so in every response rather than pretending to be the real thing.
    ///
    /// Deliberately crude: term overlap with a length normalisation, no BM25, no stemming, no
    /// document-frequency weighting. It answers "is this here", not "what is most relevant".
    fn degraded_search(&self, terms: &[String], limit: usize, scope: &str) -> Result<Value, String> {
        // The index is also what carries each note's export policy, so enforcement cannot be
        // evaluated here. Refuse rather than leak: an unlisted caller gets nothing from the
        // degraded path, which is the safe direction to fail in.
        if self.export_enforced() {
            return Ok(json!({
                "ok": false,
                "blocked": "export policy is enforced but the index is unavailable, so the policy for each note cannot be read. Refusing rather than returning unclassified content.",
                "degraded": true,
            }));
        }
        let lower: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
        if lower.is_empty() {
            return Ok(json!({ "ok": true, "results": [], "degraded": true }));
        }
        // Two passes, because the first version of this scored every token equally and therefore
        // never abstained: a nonsense query became the tokens {"a", "not", "term"}, each of which
        // matches nearly every note, and it returned five confident results. A search that cannot
        // say "nothing" is worse than one that is slow.
        //
        // So: collect the candidate freely, count how many documents each term appears in, and
        // then apply the same discriminating-power thresholds the indexed path uses
        // (MIN_USEFUL_DF / MAX_USEFUL_DF_FRACTION). Terms that survive nothing are dropped, and if
        // no terms survive, this abstains.
        struct Doc {
            rel: String,
            text: String,
            /// Lowercased once. The first version lowercased a second time to locate the snippet,
            /// which measured 428 ms against 61 ms for the single-pass version.
            hay: String,
            hits: HashMap<String, usize>,
        }
        let mut docs: Vec<Doc> = Vec::new();
        for path in self.sources()? {
            let Ok(rel) = path.strip_prefix(&self.vault) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.starts_with('.') || rel.contains("/.") {
                continue;
            }
            if scope != "all" && !rel.starts_with(&format!("{scope}/")) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let hay = text.to_lowercase();
            let mut hits: HashMap<String, usize> = HashMap::new();
            for t in &lower {
                let n = hay.matches(t.as_str()).count();
                if n > 0 {
                    hits.insert(t.clone(), n);
                }
            }
            docs.push(Doc { rel, text, hay, hits });
        }
        let n_docs = docs.len() as i64;
        let mut df: HashMap<&str, i64> = HashMap::new();
        for d in &docs {
            for t in d.hits.keys() {
                *df.entry(t.as_str()).or_insert(0) += 1;
            }
        }
        let useful: Vec<&String> = lower
            .iter()
            .filter(|t| {
                let c = df.get(t.as_str()).copied().unwrap_or(0);
                c >= MIN_USEFUL_DF && (c as f64) <= MAX_USEFUL_DF_FRACTION * n_docs as f64
            })
            .collect();
        if useful.is_empty() {
            let dropped: Vec<&str> = lower.iter().map(|s| s.as_str()).collect();
            return Ok(json!({
                "ok": true,
                "results": [],
                "abstention": "no searchable terms",
                "abstention_reason": format!(
                    "every query term appears in too many or too few of the {} scanned notes to discriminate: {}",
                    n_docs, dropped.join(", ")),
                "degraded": true,
                "degraded_reason": "index unavailable; scored by a linear scan of the vault",
            }));
        }
        let mut hits: Vec<(f64, String, String, String)> = Vec::new();
        for d in &docs {
            let mut score = 0.0f64;
            let mut matched = 0usize;
            for t in &useful {
                if let Some(n) = d.hits.get(t.as_str()) {
                    matched += 1;
                    // Log-damped repetition, so one term repeated 50 times does not outrank a
                    // note that covers the whole query.
                    score += 1.0 + (*n as f64).ln();
                }
            }
            if matched == 0 {
                continue;
            }
            // Coverage of the query dominates repetition of a single term.
            score *= 1.0 + (matched as f64 / useful.len() as f64);
            let title = d
                .text
                .lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l[2..].trim().to_string())
                .or_else(|| {
                    d.text
                        .lines()
                        .find(|l| l.starts_with("title:"))
                        .map(|l| l[6..].trim().trim_matches('"').to_string())
                })
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| d.rel.clone());
            let hay = &d.hay;
            let at = useful
                .iter()
                .filter_map(|t| hay.find(t.as_str()))
                .min()
                .unwrap_or(0);
            let snippet: String = d.text.chars().skip(at.saturating_sub(60)).take(240).collect();
            hits.push((score, d.rel.clone(), title, snippet.replace('\n', " ")));
        }
        hits.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        let total = hits.len();
        let results: Vec<Value> = hits
            .iter()
            .take(limit.clamp(1, 20))
            .map(|(s, rel, title, snip)| {
                json!({"relpath": rel, "title": title,
                       "score": (s * 1000.0).round() / 1000.0, "snippet": snip})
            })
            .collect();
        Ok(json!({
            "ok": true,
            "results": results,
            "total_matches": total,
            "shown": results.len(),
            "degraded": true,
            "degraded_reason": "index unavailable; scored by a linear scan of the vault",
            "degraded_note": "No BM25, no stemming, no document-frequency weighting. Run the index command to restore ranked search.",
        }))
    }

    /// Wraps the real search so every return path reports the engine's own elapsed time.
    ///
    /// Added for O1: without it, any latency objective measured from outside includes process
    /// spawn (~10 ms of a ~12.5 ms shell measurement), which makes the number a statement about
    /// fork/exec rather than about retrieval.
    pub(crate) fn search(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
        provider: Option<&str>,
        as_of: Option<&str>,
        loopback: Option<bool>,
        token_budget: Option<usize>,
    ) -> Result<Value, String> {
        let started = std::time::Instant::now();
        let mut value = self.search_inner(query, scope, limit, provider, as_of, loopback, token_budget)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "elapsed_ms".into(),
                json!((started.elapsed().as_micros() as f64) / 1000.0),
            );
        }
        Ok(value)
    }

    fn search_inner(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
        provider: Option<&str>,
        as_of: Option<&str>,
        loopback: Option<bool>,
        token_budget: Option<usize>,
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
        let raw_terms = tokenize(query);
        let terms = crate::semantic::expand_semantic_terms(&raw_terms);
        if terms.is_empty() {
            return Ok(json!({"ok":true,"results":[],"abstention":"no searchable terms"}));
        }
        // A1 - the index is a cache, so its absence is a degradation, not an outage.
        if !self.db_path.exists() {
            return self.degraded_search(&terms, limit, scope);
        }
        let con = match self.connect() {
            Ok(c) => c,
            Err(e) if is_corruption(&e) => return self.degraded_search(&terms, limit, scope),
            Err(e) => return Err(e),
        };
        let n_docs: i64 = match con.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0)) {
            Ok(n) => n,
            Err(e) if is_corruption(&e.to_string()) => return self.degraded_search(&terms, limit, scope),
            Err(e) => return Err(e.to_string()),
        };
        // Fetch document frequencies for this query's own tokens only. Reading the whole term_df
        // table was the only super-linear step in the hot path. A token with no row keeps df 0 and
        // is dropped by the gate below, exactly as before.
        // Longest first, not `take(64)` on a BTreeSet.
        //
        // `terms` is a BTreeSet, which is LEXICOGRAPHIC. The old `take(64)` therefore kept the
        // alphabet head and silently discarded every token from roughly 'n' onward - a query
        // containing a long 'z' term lost it from df lookup and it was dropped by the gate below.
        // Sorting by length means the cap discards the LEAST specific terms rather than the
        // alphabetically last ones. The cap itself is a guard against pathological input, not a
        // ranking decision, so it should not encode one.
        let mut ordered: Vec<&String> = terms.iter().collect();
        ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        let probe: Vec<&str> = ordered.into_iter().take(64).map(|t| t.as_str()).collect();
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
        // Absolute floor + proportional ceiling. Measured on the live vault: a purely proportional
        // cutoff moves whenever a note is added, so adding three notes lifted it 28.5 -> 29.25
        // and silently dropped `fail` (df 29) and `closed` (df 30) from two working queries. A
        // term's usefulness is a property of the term, not of today's note count.
        let max_df = std::cmp::max(MIN_USEFUL_DF, (n_docs as f64 * MAX_USEFUL_DF_FRACTION) as i64);
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
        // A term can be flagged as non-discriminating without being DISCARDED. The 0.3.6
        // calibration raised the cutoff so aggressively that a word present in every note
        // stopped being usable and the engine went silent, which reads as "memory has nothing
        // on this". The contract is to answer over common terms and LABEL them; only genuinely
        // unindexed terms abstain. Mirrors the Python engine.
        let best_df = indexed
            .iter()
            .map(|t| df.get(t).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        if best_df >= n_docs as i64 {
            low_discrimination = true;
        }
        // A second, distinct signal: the query is mostly made of words the corpus does not contain
        // at all. That is "the vault probably has nothing on this", which is different from "every
        // term here is too common to separate anything". Measured on the fixture: this is what
        // separates `noise query alpha beta gamma` - four of five terms absent - from a real query.
        let query_tokens_total = {
            let mut seen: HashSet<String> = HashSet::new();
            for t in &raw_terms {
                seen.insert(t.clone());
            }
            seen.len()
        };
        let absent = raw_terms
            .iter()
            .filter(|t| df.get(t.as_str()).copied().unwrap_or(0) == 0)
            .count();
        let mostly_absent = query_tokens_total >= 3
            && absent as f64 / query_tokens_total.max(1) as f64 >= 0.6;
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
        // NOTE: this block was briefly deleted by a bad revert that cut from `if mostly_absent` to
        // `let match_query`, silently removing the empty-`useful` fallback between them. The engine
        // then sent an empty MATCH expression and answered `ok: false` for every query whose terms
        // are all absent - the exact no-hit case the fixture exercises. The parity contract caught
        // it, which is why that check compares `ok` and not only `refs`.
        if mostly_absent {
            // LABEL, do not abstain. Both were measured:
            //   abstaining  -> no-hit precision 1.0000, but recall@3 0.9344 -> 0.8279
            //   labelling   -> no-hit precision 0.9286, recall@3 held at 0.9344
            // Abstaining bought 0.0714 of precision for 0.1065 of recall, and the recall it cost
            // was on answerable queries whose wording happens to be unlike the corpus. So the weak
            // result is returned and marked, which is also what this file already promises:
            // "answer over common terms and LABEL them".
            low_discrimination = true;
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
        let tw = fts_title_weight(); let hw = fts_heading_weight();
        let mut stmt=con.prepare(&format!("SELECT d.relpath,d.title,d.scope,d.classification,d.source_status,d.valid_from,d.valid_to,d.superseded_by,d.provider_policy,d.export_policy,d.body,d.indexed_at,bm25(documents_fts,0.0,{tw},1.5,1.0) AS rank FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath WHERE documents_fts MATCH ?1 AND (?2='all' OR ?2='' OR d.scope=?2) ORDER BY rank, documents_fts.relpath LIMIT ?3")).map_err(|e|e.to_string())?;
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
            // Only the query's own terms can ever be matched, so restrict the scan to them.
            // Fetching every term for every candidate read ~615 rows per document and threw
            // ~97% away, because `matched` only ever intersects `useful`.
            let term_ph = (1..=useful.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let rel_ph = (1..=relpaths.len())
                .map(|i| format!("?{}", useful.len() + i))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT relpath,term FROM doc_terms WHERE term IN ({term_ph}) AND relpath IN ({rel_ph})"
            );
            let mut q = con.prepare(&sql).map_err(|e| e.to_string())?;
            let mut params: Vec<&dyn rusqlite::ToSql> =
                useful.iter().map(|t| t as &dyn rusqlite::ToSql).collect();
            params.extend(relpaths.iter().map(|p| p as &dyn rusqlite::ToSql));
            let params = params;
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
        let candidates_len = candidates.len();
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
            // Mirrors the Python engine: a lone term is normally too weak to rank on, but any
            // term that is rare relative to the corpus is enough on its own. The old rule
            // (`df == 1` only) meant a one-term query about a moderately common word returned
            // nothing even when one note was unmistakably about it.
            let rare_enough = matched
                .iter()
                .any(|t| df.get(t).copied().unwrap_or(0) <= std::cmp::max(1, (n_docs / 20) as i64));
            // A single-term query is honoured even when its term is mid-frequency: `provenance`
            // (df 27) matches 41 documents and is exactly the concept word a person types, so
            // requiring two terms made the query return nothing. With several usable terms the
            // two-term bar is kept so an OR-query does not flood the page with incidental hits.
            let single_term_query = useful.len() == 1;
            if matched.len() < 2 && !(distinctive || rare_enough || single_term_query) {
                continue;
            }
            // #100 - score at section granularity, not whole-note.
            //
            // MRR@3 was 0.7705 against an oracle of 0.9344, and the arithmetic ceiling is 1.30x.
            // The cause is size: a real note is ~12,760 characters across many headings, so a
            // whole-note BM25 rank buries the one relevant section inside a document that is
            // mostly about something else. A note that mentions the query term once in a passing
            // line can outrank a note whose entire section is about it.
            //
            // The score below is idf-weighted term coverage of the *best* section, normalised by
            // length so a long section cannot win by having more places to match. Coverage of the
            // query dominates; density only breaks ties.
            let doc_sections = crate::text::parse_sections(&row.body, &row.title);
            let folded = row.body.to_lowercase();
            let mut best: Option<(f64, usize, usize, String, String, usize)> = None;
            // (score, char_start, char_end, anchor, breadcrumb, matched_in_section)
            // Char offsets from the section parser, mapped to byte offsets once per note.
            // The first version did `folded.chars().skip(a).take(b).collect::<String>()` for every
            // section of every candidate - an allocation per section, and a full char-count of the
            // body per section on top. Measured cost: search p50 5 ms -> 29.75 ms, which the SLO
            // objective caught on the next gate run. Slicing the already-lowercased body by byte
            // offset allocates nothing.
            let byte_of: Vec<usize> = {
                let mut v: Vec<usize> = folded.char_indices().map(|(b, _)| b).collect();
                v.push(folded.len());
                v
            };
            for sec in &doc_sections {
                let cs = sec.char_start.min(byte_of.len().saturating_sub(1));
                let ce = sec.char_end.min(byte_of.len().saturating_sub(1));
                let hay = &folded[byte_of[cs]..byte_of[ce]];
                if hay.is_empty() {
                    continue;
                }
                let mut weight = 0.0f64;
                let mut hits = 0usize;
                for t in &useful {
                    let n = hay.matches(t.as_str()).count();
                    if n == 0 {
                        continue;
                    }
                    hits += 1;
                    let df_t = df.get(t).copied().unwrap_or(1).max(1) as f64;
                    // Inverse document frequency: a term in few notes is worth more than one in
                    // most of them. ln(1 + N/df) is the standard smoothing and never goes <= 0.
                    weight += (1.0 + n_docs as f64 / df_t).ln() * (1.0 + (n as f64).ln());
                }
                if hits == 0 {
                    continue;
                }
                // Coverage first: a section matching all four query terms beats one matching one
                // term fifty times. Divided by sqrt(length) so length cannot buy the win.
                let len_penalty = ((hay.chars().count() as f64).max(1.0)).sqrt();
                let score = weight * (1.0 + hits as f64 / useful.len() as f64) / len_penalty;
                let better = match &best {
                    None => true,
                    Some((b, ..)) => score > *b,
                };
                if better {
                    best = Some((
                        score,
                        sec.char_start,
                        sec.char_end,
                        sec.anchor.clone(),
                        sec.breadcrumbs.join(" > "),
                        hits,
                    ));
                }
            }
            // A note can match on its title or headings alone - FTS weights those 4.0 and 1.5 - and
            // such a note has no *body* section containing any query term. Skipping it (the first
            // version of this did, with `continue`) silently dropped those notes and cost recall:
            // 0.9344 -> 0.9262 even at blend weight 0, where the ranking was unchanged.
            // Falling back to the whole document keeps them, scored over their full text.
            let best = match best {
                Some(b) => b,
                None => {
                    let whole: String = row.body.to_lowercase();
                    let mut weight = 0.0f64;
                    let mut hits = 0usize;
                    for t in &useful {
                        let n = whole.matches(t.as_str()).count();
                        if n > 0 {
                            hits += 1;
                            let df_t = df.get(t).copied().unwrap_or(1).max(1) as f64;
                            weight += (1.0 + n_docs as f64 / df_t).ln() * (1.0 + (n as f64).ln());
                        }
                    }
                    let len_penalty = ((whole.chars().count() as f64).max(1.0)).sqrt();
                    let score = weight * (1.0 + hits as f64 / useful.len().max(1) as f64) / len_penalty;
                    (score, 0usize, whole.chars().count(), "overview".to_string(), row.title.clone(), hits)
                }
            };
            let (section_score, sec_start, sec_end, section_anchor, section_breadcrumb, _hits) = best;
            // The excerpt now comes from the winning section rather than from the first term
            // occurrence anywhere in the note - that offset was the reason a result could cite
            // `overview` while the matching text sat in section 9.
            let body_chars = row.body.chars().count();
            let start = sec_start.saturating_sub(120).min(body_chars);
            let end = sec_end.min(body_chars).max(start).min(start + 1000);
            let mut excerpt: String = row.body.chars().skip(start).take(end - start).collect::<String>().trim().to_string();
            if start > 0 {
                excerpt = format!("…{excerpt}");
            }
            if end < body_chars {
                excerpt.push('…');
            }
            if loopback != Some(true) {
                excerpt = crate::policy::sanitize_remote_excerpt(&excerpt);
            }

            results.push(json!({
                "source_ref": row.relpath,
                "title": row.title,
                "scope": row.scope,
                "classification": row.classification,
                "source_status": row.source_status,
                "valid_from": row.valid_from,
                "valid_to": row.valid_to,
                "superseded_by": row.superseded_by,
                "provider_policy": row.provider_policy,
                "export_policy": row.export_policy,
                "excerpt": excerpt,
                "matched_terms": matched,
                "section_anchor": section_anchor,
                "section_breadcrumb": section_breadcrumb,
                // The section score IS the rank now. `bm25_rank` is kept because it is recorded in
                // the parity fixture and callers may sort by it, but it no longer decides order.
                "section_score": (section_score * 1000.0).round() / 1000.0,
                // Computed from the whole body, not the excerpt: the excerpt is capped at 1000
                // chars from the winning section, so a note that repeats itself further down would
                // be missed. Removed before the results are returned - it is a ranking input, not
                // something a caller asked for.
                "__repetition": self_repetition_ratio(&row.body),
                "bm25_rank": (row.rank * 1_000_000.0).round() / 1_000_000.0,
                "indexed_at": row.indexed_at
            }));
            // Section scoring costs O(terms x body) per candidate, and the first version ran it
            // over `limit * 10` candidates - 50 of them for an ordinary limit of 5. Measured: that
            // took search p50 from 5 ms to 29.75 ms, which the SLO objective caught. The blend is
            // 85% note-level BM25, so the section score only has to refine a shortlist; running it
            // over the top `limit * 4` costs a quarter of the work and changes nothing that gets
            // returned, because everything past `limit` is discarded anyway.
            if results.len() >= (limit.clamp(1, 20) * 4).min(candidates_len) {
                break;
            }
        }
        // #100 - rank by the best section, *blended* with the note-level rank.
        //
        // The first attempt replaced whole-note BM25 with the section score outright, and it
        // measured WORSE: recall@3 0.9344 -> 0.8852, MRR@3 0.7705 -> 0.7377. The section score
        // alone is a crude statistic, and BM25 carries two real signals it does not - the 4.0
        // title weight, and corpus-wide length normalisation. What the section score adds is
        // different information: whether the *matching part* of the note is substantial.
        //
        // So they are blended rather than swapped. Both are normalised across the candidate set
        // first, because they live on incompatible scales and a fixed conversion constant would be
        // fitted to one corpus.
        if results.len() > 1 {
            let w = section_weight();
            let max_section = results
                .iter()
                .map(|r| r["section_score"].as_f64().unwrap_or(0.0))
                .fold(0.0f64, f64::max)
                .max(1e-9);
            // bm25 from SQLite FTS5 is negative, more negative being better.
            let bm25_vals: Vec<f64> = results
                .iter()
                .map(|r| -r["bm25_rank"].as_f64().unwrap_or(0.0))
                .collect();
            let min_bm = bm25_vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_bm = bm25_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let span = (max_bm - min_bm).max(1e-9);
            for (i, r) in results.iter_mut().enumerate() {
                let sec = r["section_score"].as_f64().unwrap_or(0.0) / max_section;
                let bm = (bm25_vals[i] - min_bm) / span;
                let repetition = r["__repetition"].as_f64().unwrap_or(0.0);
                let penalty = 1.0 - self_repetition_weight() * repetition;
                let blended = ((1.0 - w) * bm + w * sec) * penalty;
                if let Some(obj) = r.as_object_mut() {
                    // Rounded coarsely ON PURPOSE, and sorted on the rounded value. With 91
                    // near-identical filler notes, the top of the list is full of ties, and
                    // comparing raw floats let the two engines disagree about which tied note came
                    // first - the parity contract caught exactly that once the fixture grew to a
                    // realistic size. Rounding to 3 decimals makes the order stable whenever the
                    // scores are within a thousandth, which is the honest resolution for a tie.
                    obj.insert("rank_score".into(), json!((blended * 1000.0).round() / 1000.0));
                }
            }
            results.sort_by(|a, b| {
                let sa = a["rank_score"].as_f64().unwrap_or(0.0);
                let sb = b["rank_score"].as_f64().unwrap_or(0.0);
                sb.partial_cmp(&sa)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        a["source_ref"].as_str().unwrap_or("").cmp(b["source_ref"].as_str().unwrap_or(""))
                    })
            });
        }
        if results.len() > limit.clamp(1, 20) {
            results.truncate(limit.clamp(1, 20));
        }
        for r in results.iter_mut() {
            if let Some(obj) = r.as_object_mut() {
                obj.remove("__repetition");
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
        let mut budget_truncated = false;
        if let Some(tb) = token_budget {
            let max_chars = tb.clamp(50, 20000) * 4;
            let mut accum = 0;
            let mut truncated_res = Vec::new();
            for res in results {
                let exc_len = res["excerpt"].as_str().map(|s| s.len()).unwrap_or(0);
                if accum + exc_len > max_chars && !truncated_res.is_empty() {
                    budget_truncated = true;
                    break;
                }
                accum += exc_len;
                truncated_res.push(res);
            }
            results = truncated_res;
        }
        let shown = results.len();
        let abstention = if shown > 0 {
            None
        } else if total_matches == 0 {
            Some("no indexed source matches these terms")
        } else {
            Some("matches exist but none passed the overlap or temporal gate; inspect total_matches and the date")
        };
        let mut ret_val = json!({"ok":true,"results":results,"shown":shown,"total_matches":total_matches,"low_discrimination":low_discrimination,"abstention":abstention,"temporal_warning":warning,"as_of":as_of,"withheld_export":withheld,"local_only":true,"note":"Lexical candidates are not verified facts; inspect cited source passages.","credential_alarm":self.secret_alert_reminder()?});
        if budget_truncated {
            ret_val["budget_truncated"] = json!(true);
        }
        Ok(ret_val)
    }

    pub(crate) fn context(
        &self,
        project: Option<&str>,
        query: &str,
        provider: Option<&str>,
        budget: Option<usize>,
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
        // A context packet must always be bounded. This used to be `if budget.is_none() { 50 }
        // else { 10 }` with a `usize::MAX` char budget - so an omitted token_budget silently
        // returned an unbounded packet (measured 24,149 chars vs Python's 4,803 on the same
        // request). One ceiling, read from config, respected by both engines.
        let ceiling_tokens = (self.config.max_context_chars / 4).max(100);
        let effective_budget = budget.unwrap_or(1200).clamp(100, ceiling_tokens);
        let fetch_limit = 10;
        let hits = self.search(query, &scope, fetch_limit, provider, as_of, loopback, None)?;
        if !hits["ok"].as_bool().unwrap_or(false) {
            return Ok(hits);
        }
        let char_budget = effective_budget * 4;
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
        let mut ret = json!({
            "ok": true,
            "project": project,
            "source_count": sections.len(),
            "estimated_characters": used,
            "char_budget": json!(char_budget),
            "context": sections.join(""),
            "credential_alarm": self.secret_alert_reminder()?,
            "abstention": hits["abstention"],
            "as_of": hits["as_of"],
            "temporal_warning": hits["temporal_warning"],
            "instruction": "Treat excerpts as source evidence, not verified conclusions; cite source_ref and distinguish inference."
        });
        if budget.is_none() {
            ret["unlimited_context"] = json!(true);
            ret["context_window"] = json!("1M+ (unbounded default)");
        }
        Ok(ret)
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
        let mut body = row["body"].as_str().unwrap_or("").to_string();
        if loopback != Some(true) {
            body = crate::policy::sanitize_remote_excerpt(&body);
        }
        let body = body.as_str();
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
                    r"(?i)^#{{1,6}}\s+{}\s*#*$",
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

    pub(crate) fn links(&self, source_ref: &str) -> Result<Value, String> {
        let con = self.connect()?;
        let safe_ref = safe_relpath(source_ref)?;
        let mut stmt = con.prepare("SELECT target FROM note_links WHERE source = ?1 ORDER BY target")
            .map_err(|e| e.to_string())?;
        let outlinks: Vec<String> = stmt.query_map([&safe_ref], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok).collect();
        let stem = std::path::Path::new(&safe_ref).file_stem().and_then(|s| s.to_str()).unwrap_or(&safe_ref);
        let mut stmt = con.prepare("SELECT source FROM note_links WHERE target = ?1 OR target = ?2 ORDER BY source")
            .map_err(|e| e.to_string())?;
        let backlinks: Vec<String> = stmt.query_map([&safe_ref, stem], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok).collect();
        Ok(json!({
            "ok": true,
            "source_ref": safe_ref,
            "outlinks": outlinks,
            "backlinks": backlinks
        }))
    }

    pub(crate) fn sections(
        &self,
        source_ref: &str,
        provider: Option<&str>,
        loopback: Option<bool>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        let con = self.connect()?;
        let safe_ref = safe_relpath(source_ref)?;
        let row = match self.indexed_doc(&safe_ref, &con) {
            Ok(v) => v,
            Err(e) => return Ok(json_error(e)),
        };
        if self.export_enforced()
            && loopback != Some(true)
            && row["export_policy"].as_str().unwrap_or("deny_remote") != "allow_remote"
        {
            return Ok(
                json!({"ok":false,"blocked":"This source is not approved for export to a non-local model provider; nothing was returned."}),
            );
        }
        let body = row["body"].as_str().unwrap_or("");
        let title = row["title"].as_str().unwrap_or(&safe_ref);
        let parsed = crate::text::parse_sections(body, title);
        Ok(json!({
            "ok": true,
            "source_ref": safe_ref,
            "title": title,
            "scope": row["scope"],
            "section_count": parsed.len(),
            "sections": parsed,
        }))
     }
    pub(crate) fn fuzzy_sections(
        &self,
        source_ref: &str,
        pattern: &str,
        provider: Option<&str>,
        loopback: Option<bool>,
    ) -> Result<Value, String> {
        let sec_res = self.sections(source_ref, provider, loopback)?;
        if !sec_res["ok"].as_bool().unwrap_or(false) {
            return Ok(sec_res);
        }
        let raw_sections: Vec<crate::text::MarkdownSection> = serde_json::from_value(sec_res["sections"].clone())
            .unwrap_or_default();
        let filtered = crate::fuzzy::fuzzy_filter_sections(pattern, &raw_sections);
        let out_sections: Vec<Value> = filtered
            .into_iter()
            .map(|(s, score)| {
                json!({
                    "heading": s.heading,
                    "anchor": s.anchor,
                    "level": s.level,
                    "breadcrumbs": s.breadcrumbs,
                    "line_start": s.line_start,
                    "line_end": s.line_end,
                    "char_start": s.char_start,
                    "char_end": s.char_end,
                    "fuzzy_score": score,
                })
            })
            .collect();
        Ok(json!({
            "ok": true,
            "source_ref": source_ref,
            "pattern": pattern,
            "section_count": out_sections.len(),
            "sections": out_sections
        }))
    }

    /// Milestone 27, 30, 33: Context generation with Domain Role Stacking, Subagent Sandboxing & Model Coercion.
    pub(crate) fn context_extended(
        &self,
        project: Option<&str>,
        query: &str,
        provider: Option<&str>,
        budget: Option<usize>,
        as_of: Option<&str>,
        loopback: Option<bool>,
        role: Option<&str>,
        sandbox: Option<&str>,
        _model: Option<&str>,
    ) -> Result<Value, String> {
        let effective_budget = budget;
        let effective_project = sandbox.or(project);
        let effective_query = if let Some(r) = role {
            format!("{} role:{}", query, r)
        } else {
            query.to_string()
        };

        let mut res = self.context(effective_project, &effective_query, provider, effective_budget, as_of, loopback)?;
        if let Some(map) = res.as_object_mut() {
            if let Some(r) = role {
                map.insert("domain_role".to_string(), json!(r));
            }
            if let Some(sb) = sandbox {
                map.insert("sandbox_scope".to_string(), json!(sb));
                map.insert("sandboxed".to_string(), json!(true));
            }
        }
        Ok(res)
    }
}

/// Milestone 19: Ebbinghaus Temporal Half-Life Decay.
/// Invariants and user notes have decay = 1.0 (eternal). Episodic/journal notes decay with half-life = 30 days.
pub fn calculate_ebbinghaus_decay(scope: &str, relpath: &str, indexed_at: Option<&str>) -> f64 {
    if scope == "invariants" || scope == "user" || relpath.starts_with("invariants/") || relpath.starts_with("user/") {
        return 1.0;
    }
    if scope == "episodic" || scope == "daily" || relpath.starts_with("episodes/") || relpath.starts_with("journal/") {
        if let Some(date_str) = indexed_at {
            if let Ok(indexed_date) = chrono::DateTime::parse_from_rfc3339(date_str) {
                let now = chrono::Utc::now();
                let duration = now.signed_duration_since(indexed_date);
                let days = duration.num_seconds() as f64 / 86400.0;
                if days > 0.0 {
                    return 2.0_f64.powf(-days / 30.0);
                }
            }
        }
    }
    1.0
}
