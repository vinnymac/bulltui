//! A tiny, dependency-free fuzzy matcher and filter expression.
//!
//! The corpus is always small (a page of jobs, a handful of queues/commands),
//! so a ~50-line subsequence scorer beats pulling in a matcher crate with its
//! own threading/config model — and it stays deterministic, which the render
//! path requires.

/// Case-insensitive subsequence score of `needle` against `haystack`.
///
/// `None` when `needle` is not a subsequence of `haystack`. Higher is better:
/// matches at word starts and runs of consecutive matches score more, so
/// "send" ranks `send-email` above `a-resend`.
pub fn score(needle: &str, haystack: &str) -> Option<i32> {
    let need: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    if need.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().flat_map(|c| c.to_lowercase()).collect();

    let mut hi = 0usize;
    let mut total = 0i32;
    let mut prev: Option<usize> = None;
    for &nc in &need {
        let mut found = None;
        while hi < hay.len() {
            if hay[hi] == nc {
                found = Some(hi);
                break;
            }
            hi += 1;
        }
        let idx = found?;
        let mut pts = 1;
        if let Some(p) = prev {
            if idx == p + 1 {
                pts += 8; // consecutive run dominates
            } else {
                pts -= (idx - p - 1).min(4) as i32; // capped gap penalty
            }
        }
        if idx == 0 || !hay[idx - 1].is_alphanumeric() {
            pts += 4; // word-start bonus
        }
        total += pts;
        prev = Some(idx);
        hi = idx + 1;
    }
    Some(total)
}

/// Whether `needle` fuzzy-matches `haystack`.
pub fn is_match(needle: &str, haystack: &str) -> bool {
    score(needle, haystack).is_some()
}

/// A `/`-filter expression: whitespace-separated terms, all of which must hold
/// (logical AND). A term prefixed with `!` is *negated* — the (case-insensitive)
/// substring must NOT appear. Positive terms use fuzzy subsequence matching.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    positive: Vec<String>,
    negative: Vec<String>,
}

impl Filter {
    pub fn parse(query: &str) -> Filter {
        let mut f = Filter::default();
        for term in query.split_whitespace() {
            if let Some(rest) = term.strip_prefix('!') {
                if !rest.is_empty() {
                    f.negative.push(rest.to_lowercase());
                }
            } else {
                f.positive.push(term.to_string());
            }
        }
        f
    }

    pub fn is_empty(&self) -> bool {
        self.positive.is_empty() && self.negative.is_empty()
    }

    /// Whether `text` satisfies every term.
    pub fn matches(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        if self.negative.iter().any(|n| lower.contains(n)) {
            return false;
        }
        self.positive.iter().all(|p| is_match(p, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_hit_and_miss() {
        assert!(is_match("snd", "send-email"));
        assert!(is_match("", "anything"));
        assert!(!is_match("xyz", "send-email"));
        assert!(!is_match("eee", "send"));
    }

    #[test]
    fn consecutive_and_word_start_rank_higher() {
        // exact prefix beats a scattered match
        let prefix = score("send", "send-email").unwrap();
        let scattered = score("send", "a-s-e-n-d").unwrap();
        assert!(prefix > scattered, "{prefix} !> {scattered}");
        // a word-start match beats a mid-word one
        let at_start = score("e", "x-email").unwrap();
        let mid = score("e", "name").unwrap();
        assert!(at_start > mid, "{at_start} !> {mid}");
    }

    #[test]
    fn filter_parses_negation_and_ands() {
        let f = Filter::parse("send !fail");
        assert!(f.matches("send-email completed"));
        assert!(!f.matches("send-email failed")); // negated "fail" present
        assert!(!f.matches("recv-email")); // positive "send" absent

        assert!(Filter::parse("   ").is_empty());
        // negation only
        let n = Filter::parse("!error");
        assert!(n.matches("ok"));
        assert!(!n.matches("had an ERROR"));
    }
}
