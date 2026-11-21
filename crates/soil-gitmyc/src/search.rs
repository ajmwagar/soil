//! Searching content, and the one thing that is NOT an index.
//!
//! A per-file Bloom filter over trigrams looks like a cheap prefilter and is not
//! one: 512 bits saturates at roughly 5 KB of text (fill 0.997 at 1000 distinct
//! trigrams, 1.000 at 4000), and the measured result was 134 of 212 files kept as
//! "candidates" for a pattern that appears in none of them. A filter that keeps two
//! thirds of a corpus costs a hydration per file and buys nothing.
//!
//! So this module is deliberately small: a literal line scan to *confirm* a hit, and
//! the predicate that decides whether a file is text at all. A real index is a
//! corpus-level structure (trigram → postings, or embeddings) built where the bytes
//! already are — which is `soil:index`'s job, not a manifest's.

/// One confirmed hit: a file that was read, and where the pattern is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub line: u64,
    /// The line, truncated so a hit list stays readable.
    pub text: String,
}

/// Literal, line-oriented scan. This is the *confirmation* step of a search, not the
/// search itself: a real index narrows the corpus first, and this proves the hits.
#[must_use]
pub fn scan(path: &str, bytes: &[u8], pattern: &str, max_hits: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    if pattern.is_empty() || max_hits == 0 {
        return hits;
    }
    let needle = pattern.as_bytes();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.len() >= needle.len() && line.windows(needle.len()).any(|window| window == needle) {
            hits.push(Hit {
                path: path.to_string(),
                line: index as u64 + 1,
                text: String::from_utf8_lossy(&line[..line.len().min(200)]).to_string(),
            });
            if hits.len() >= max_hits {
                break;
            }
        }
    }
    hits
}

/// Text-ish by media type. A search that hydrates a PNG to grep it is a search that
/// wasted a fetch.
#[must_use]
pub fn is_searchable(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json" | "application/xml" | "application/javascript"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_confirms_and_locates() {
        let text = b"# Plan\n\n- alfalfa tests\n- other\n- alfalfa again\n";
        let hits = scan("PLAN.md", text, "alfalfa", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 3);
        assert_eq!(hits[1].line, 5);
        assert!(hits[0].text.contains("alfalfa tests"));
    }

    #[test]
    fn scanning_is_bounded_and_ignores_short_lines() {
        let text = b"x\ny\nx\ny\nx\ny\n";
        assert_eq!(scan("f", text, "x", 2).len(), 2);
        assert!(scan("f", b"a\nbb\n", "abc", 5).is_empty());
        assert!(scan("f", b"anything", "", 5).is_empty());
    }

    #[test]
    fn only_text_is_worth_searching() {
        assert!(is_searchable("text/plain"));
        assert!(is_searchable("application/json"));
        assert!(!is_searchable("image/png"));
        assert!(!is_searchable("application/wasm"));
        assert!(!is_searchable("application/octet-stream"));
    }
}
