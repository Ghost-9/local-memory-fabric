use crate::text::MarkdownSection;

/// Computes a 64-bit character mask for rapid pre-filtering.
/// Maps ASCII characters 'a'..'z', '0'..'9', and common punctuation to 64 discrete bit slots.
pub fn char_mask(s: &str) -> u64 {
    let mut mask: u64 = 0;
    for b in s.bytes() {
        let bit = match b.to_ascii_lowercase() {
            c @ b'a'..=b'z' => c - b'a',        // bits 0..25
            d @ b'0'..=b'9' => 26 + (d - b'0'), // bits 26..35
            b'_' => 36,
            b'-' => 37,
            b'.' => 38,
            b'/' => 39,
            b':' => 40,
            b' ' => 41,
            b'#' => 42,
            _ => 63, // catch-all bit 63
        };
        mask |= 1u64 << bit;
    }
    mask
}

/// Zero-copy masked fuzzy matcher.
/// Returns Some(score) if all characters in `pattern` appear in order in `target`,
/// else None.
pub fn masked_fuzzy_score(pattern: &str, target: &str) -> Option<f64> {
    if pattern.is_empty() {
        return Some(1.0);
    }
    if target.is_empty() {
        return None;
    }

    // Single-cycle bitmask reject: if target doesn't contain all distinct query chars, fail instantly.
    let pat_mask = char_mask(pattern);
    let tgt_mask = char_mask(target);
    if (tgt_mask & pat_mask) != pat_mask {
        return None;
    }

    let p_bytes: Vec<u8> = pattern.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let t_bytes: Vec<u8> = target.bytes().collect();

    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut score = 0.0;
    let mut consecutive = 0;
    let mut prev_matched_idx: Option<usize> = None;

    while p_idx < p_bytes.len() && t_idx < t_bytes.len() {
        let t_char = t_bytes[t_idx].to_ascii_lowercase();
        if t_char == p_bytes[p_idx] {
            // Base match score
            score += 10.0;

            // Prefix match bonus
            if t_idx == 0 {
                score += 15.0;
            }

            // Consecutive match bonus
            if let Some(prev) = prev_matched_idx {
                if t_idx == prev + 1 {
                    consecutive += 1;
                    score += consecutive as f64 * 5.0;
                } else {
                    consecutive = 0;
                    // Penalty for skips
                    let skip_distance = (t_idx - prev - 1) as f64;
                    score -= skip_distance.min(5.0);
                }
            }

            // Word boundary bonus
            if t_idx > 0 {
                let prev_byte = t_bytes[t_idx - 1];
                if b"_-/. :#".contains(&prev_byte)
                    || (prev_byte.is_ascii_lowercase() && t_bytes[t_idx].is_ascii_uppercase())
                {
                    score += 12.0;
                }
            }

            prev_matched_idx = Some(t_idx);
            p_idx += 1;
        }
        t_idx += 1;
    }

    if p_idx == p_bytes.len() {
        // Target length penalty to favor tighter matches
        let len_diff = (t_bytes.len() - p_bytes.len()) as f64;
        score -= len_diff * 0.1;
        let max_possible = (p_bytes.len() as f64) * 35.0;
        let normalized = (score / max_possible).clamp(0.01, 1.0);
        Some(normalized)
    } else {
        None
    }
}

/// Bitset intersection check over token vectors.
#[allow(dead_code)]
pub fn bitset_intersects(query_tokens: &[&str], target_tokens: &[&str]) -> bool {
    let mut q_mask: u64 = 0;
    for (i, &t) in query_tokens.iter().enumerate().take(64) {
        let t_lower = t.to_ascii_lowercase();
        for &tgt in target_tokens {
            if tgt.eq_ignore_ascii_case(&t_lower) {
                q_mask |= 1u64 << i;
                break;
            }
        }
    }
    q_mask.count_ones() >= query_tokens.len().min(1) as u32
}

/// Filters and ranks AST sections using masked fuzzy scoring.
pub fn fuzzy_filter_sections(
    pattern: &str,
    sections: &[MarkdownSection],
) -> Vec<(MarkdownSection, f64)> {
    let mut scored: Vec<(MarkdownSection, f64)> = Vec::new();
    let pat_clean = pattern.trim();
    if pat_clean.is_empty() {
        return sections.iter().map(|s| (s.clone(), 1.0)).collect();
    }

    for sec in sections {
        let heading_score = masked_fuzzy_score(pat_clean, &sec.heading);
        let anchor_score = masked_fuzzy_score(pat_clean, &sec.anchor);
        let breadcrumb_score = masked_fuzzy_score(pat_clean, &sec.breadcrumbs.join(" > "));

        let best_score = [heading_score, anchor_score, breadcrumb_score]
            .into_iter()
            .flatten()
            .fold(0.0f64, |acc, s| acc.max(s));

        if best_score > 0.0 {
            scored.push((sec.clone(), best_score));
        }
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_mask_rejection() {
        let pat_mask = char_mask("xyz");
        let tgt_mask = char_mask("architecture overview");
        assert_eq!(tgt_mask & pat_mask, 0);
        assert!(masked_fuzzy_score("xyz", "architecture overview").is_none());
    }

    #[test]
    fn test_masked_fuzzy_scoring() {
        let score = masked_fuzzy_score("arch", "architecture-overview");
        assert!(score.is_some());
        let s = score.unwrap();
        assert!(s > 0.5);

        let word_boundary_score = masked_fuzzy_score("ao", "architecture-overview");
        assert!(word_boundary_score.is_some());
    }

    #[test]
    fn test_section_filtering() {
        let sections = vec![
            MarkdownSection {
                heading: "Architecture Overview".into(),
                anchor: "architecture-overview".into(),
                level: 2,
                line_start: 5,
                line_end: 15,
                char_start: 100,
                char_end: 400,
                breadcrumbs: vec!["Root".into(), "Architecture Overview".into()],
            },
            MarkdownSection {
                heading: "Database Schema".into(),
                anchor: "database-schema".into(),
                level: 2,
                line_start: 16,
                line_end: 30,
                char_start: 401,
                char_end: 800,
                breadcrumbs: vec!["Root".into(), "Database Schema".into()],
            },
        ];

        let filtered = fuzzy_filter_sections("arch", &sections);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0.heading, "Architecture Overview");
    }
}
