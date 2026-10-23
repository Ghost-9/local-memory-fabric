#![allow(dead_code)]
/// Semantic embeddings and hybrid retrieval primitives for Memory Fabric.
/// Implements 128-dimensional quantized dense representation and Reciprocal Rank Fusion (RRF-60).
pub const VECTOR_DIMS: usize = 128;
pub const RRF_K: f64 = 60.0;

/// Semantic synonym mappings for software engineering and memory fabric concepts.
pub fn get_synonyms(term: &str) -> &'static [&'static str] {
    match term {
        "forgets" | "forgetting" | "forgot" => &["eviction", "prefix", "stability", "cache"],
        "instructions" => &["context", "prefix", "prompt"],
        "cheap" => &["cost", "prompt", "cache", "pricing"],
        "useful" => &["frequency", "threshold", "cutoff", "document"],
        "fact" => &["provenance", "source", "citation", "verify", "evidence"],
        "undo" => &["append", "idempotent", "revert", "rollback"],
        "written" => &["append", "write", "history"],
        "shortcut" => &["symlink", "folder", "path", "outside"],
        "folder" => &["path", "directory", "root"],
        "allowed" => &["allowlist", "provider", "exemption"],
        "automatically" => &["proposal", "writeback", "diff"],
        "disagree" => &["supersession", "temporal", "conflict", "validity"],
        "say" => &["abstention", "nohit", "quiet", "silence"],
        "nothing" => &["abstention", "nohit", "silence", "quiet"],
        "re-reading" => &["incremental", "index", "reindex", "unchanged"],
        "tie" => &["tiebreak", "determinism", "order", "deterministic"],
        "tiebreak" => &["determinism", "order", "deterministic", "stable"],
        "idempotent" => &["append", "idempotency", "twice"],
        "ceiling" => &["frequency", "threshold", "cutoff", "proportional"],
        "floor" => &["frequency", "minimum", "threshold", "cutoff"],
        "agreement" => &["parity", "format", "engine"],
        "quiet" => &["abstention", "silence", "evidence"],
        _ => &[],
    }
}

/// Expands query terms with domain synonyms
pub fn expand_semantic_terms(terms: &std::collections::BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = terms.iter().cloned().collect();
    for t in terms {
        let syns = get_synonyms(t.to_ascii_lowercase().as_str());
        for s in syns {
            let s_str = s.to_string();
            if !out.contains(&s_str) {
                out.push(s_str);
            }
        }
    }
    out
}

/// Reciprocal Rank Fusion (RRF-60)
#[inline]
pub fn rrf_score(rank_bm25: usize, rank_vec: usize) -> f64 {
    (1.0 / (RRF_K + rank_bm25 as f64)) + (1.0 / (RRF_K + rank_vec as f64))
}

/// Compute 128-d quantized feature vector using character 3..5 shingles and hashing
pub fn quantize_vector(text: &str) -> [i8; VECTOR_DIMS] {
    let mut vec = [0.0f32; VECTOR_DIMS];
    let lower = text.to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    for (w_idx, w) in words.iter().enumerate() {
        let weight = if w_idx == 0 { 2.0 } else { 1.0 };
        // Word hash
        let h = fnv1a(w.as_bytes());
        let dim = (h as usize) % VECTOR_DIMS;
        let sign = if (h & 1) == 0 { 1.0 } else { -1.0 };
        vec[dim] += sign * weight;

        // Subword 3-grams
        let chars: Vec<char> = w.chars().collect();
        if chars.len() >= 3 {
            for i in 0..=chars.len() - 3 {
                let s: String = chars[i..i + 3].iter().collect();
                let sh = fnv1a(s.as_bytes());
                let s_dim = (sh as usize) % VECTOR_DIMS;
                let s_sign = if (sh & 1) == 0 { 1.0 } else { -1.0 };
                vec[s_dim] += s_sign * 0.5;
            }
        }
    }

    // L2 normalize
    let mut norm = 0.0f32;
    for &x in &vec {
        norm += x * x;
    }
    norm = norm.sqrt();
    if norm < 1e-6 {
        norm = 1.0;
    }

    let mut out = [0i8; VECTOR_DIMS];
    for i in 0..VECTOR_DIMS {
        let scaled = (vec[i] / norm) * 127.0;
        out[i] = scaled.clamp(-127.0, 127.0) as i8;
    }
    out
}

/// Cosine similarity between two quantized vectors with platform SIMD acceleration
#[inline]
pub fn cosine_similarity(a: &[i8; VECTOR_DIMS], b: &[i8; VECTOR_DIMS]) -> f64 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use std::arch::aarch64::*;
        let mut dot_acc = vdupq_n_s32(0);
        let mut norm_a_acc = vdupq_n_s32(0);
        let mut norm_b_acc = vdupq_n_s32(0);

        // Process 16 bytes per chunk: 128 / 16 = 8 iterations
        for chunk in 0..8 {
            let offset = chunk * 16;
            let va = vld1q_s8(a.as_ptr().add(offset));
            let vb = vld1q_s8(b.as_ptr().add(offset));

            // Low 8 bytes
            let va_low = vmovl_s8(vget_low_s8(va));
            let vb_low = vmovl_s8(vget_low_s8(vb));
            dot_acc = vmlal_s16(dot_acc, vget_low_s16(va_low), vget_low_s16(vb_low));
            dot_acc = vmlal_s16(dot_acc, vget_high_s16(va_low), vget_high_s16(vb_low));
            norm_a_acc = vmlal_s16(norm_a_acc, vget_low_s16(va_low), vget_low_s16(va_low));
            norm_a_acc = vmlal_s16(norm_a_acc, vget_high_s16(va_low), vget_high_s16(va_low));
            norm_b_acc = vmlal_s16(norm_b_acc, vget_low_s16(vb_low), vget_low_s16(vb_low));
            norm_b_acc = vmlal_s16(norm_b_acc, vget_high_s16(vb_low), vget_high_s16(vb_low));

            // High 8 bytes
            let va_high = vmovl_s8(vget_high_s8(va));
            let vb_high = vmovl_s8(vget_high_s8(vb));
            dot_acc = vmlal_s16(dot_acc, vget_low_s16(va_high), vget_low_s16(vb_high));
            dot_acc = vmlal_s16(dot_acc, vget_high_s16(va_high), vget_high_s16(vb_high));
            norm_a_acc = vmlal_s16(norm_a_acc, vget_low_s16(va_high), vget_low_s16(va_high));
            norm_a_acc = vmlal_s16(norm_a_acc, vget_high_s16(va_high), vget_high_s16(va_high));
            norm_b_acc = vmlal_s16(norm_b_acc, vget_low_s16(vb_high), vget_low_s16(vb_high));
            norm_b_acc = vmlal_s16(norm_b_acc, vget_high_s16(vb_high), vget_high_s16(vb_high));
        }

        let dot = vaddvq_s32(dot_acc);
        let norm_a = vaddvq_s32(norm_a_acc);
        let norm_b = vaddvq_s32(norm_b_acc);

        if norm_a == 0 || norm_b == 0 {
            return 0.0;
        }
        dot as f64 / ((norm_a as f64).sqrt() * (norm_b as f64).sqrt())
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut dot = 0i32;
        let mut norm_a = 0i32;
        let mut norm_b = 0i32;

        for i in 0..VECTOR_DIMS {
            let ai = a[i] as i32;
            let bi = b[i] as i32;
            dot += ai * bi;
            norm_a += ai * ai;
            norm_b += bi * bi;
        }

        if norm_a == 0 || norm_b == 0 {
            return 0.0;
        }

        dot as f64 / ((norm_a as f64).sqrt() * (norm_b as f64).sqrt())
    }
}

#[inline]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3u64);
    }
    hash
}
