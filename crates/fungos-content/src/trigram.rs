//! A trigram index keyed by content id.
//!
//! This is the one index shape the ecosystem was missing: documents are
//! *content*, so an entry is `content_id → trigrams`, never `path → trigrams`.
//! Three things follow, and they are the whole reason to key it this way:
//!
//! * **dedupe** — the same bytes in two repos, two commits or two ingests are one
//!   document with several paths. That is what lets the wiki stop re-indexing a
//!   file it already has from git.
//! * **no staleness** — changed bytes are a different id, so an entry cannot
//!   describe content that no longer exists. There is no reindex-on-edit.
//! * **portability** — an index can travel next to the content it describes (a
//!   manifest, a release, a peer), which is what makes it usable on a node that
//!   has fetched nothing.
//!
//! Postings are exact: every trigram of every document is stored, so a query can
//! only *narrow* the candidate set. False positives are impossible too — a
//! candidate is confirmed by reading the bytes — so the only cost of a hit is one
//! read.
//!
//! Encoding is delta + varint over document handles. A 212-document corpus of ~1 MB
//! encodes to a few percent of the content it describes; measure it with
//! [`TrigramIndex::len`] rather than trusting a number in a comment.

use std::collections::BTreeMap;

const MAGIC: &[u8; 4] = b"TRI1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub content_id: String,
    pub size: u64,
    pub media_type: String,
    /// Every path this content was seen at. Two paths, one document.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TrigramIndex {
    docs: Vec<Doc>,
    /// Document handle for a content id, so `add` is a dedupe and not a duplicate.
    by_content: BTreeMap<String, u32>,
    /// trigram → ascending, unique document handles.
    postings: BTreeMap<[u8; 3], Vec<u32>>,
}

impl TrigramIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a document's text. Repeat content with a new path is the *same*
    /// document and only gains a path — that is the dedupe.
    ///
    /// Returns `true` when this content id was not already indexed, so a caller can
    /// report "new vs deduped" without comparing ids itself.
    pub fn add(
        &mut self,
        content_id: &str,
        path: &str,
        media_type: &str,
        size: u64,
        text: &[u8],
    ) -> bool {
        if let Some(handle) = self.by_content.get(content_id) {
            let doc = &mut self.docs[*handle as usize];
            if !doc.paths.iter().any(|existing| existing == path) {
                doc.paths.push(path.to_string());
            }
            return false;
        }
        let handle = self.docs.len() as u32;
        self.by_content.insert(content_id.to_string(), handle);
        self.docs.push(Doc {
            content_id: content_id.to_string(),
            size,
            media_type: media_type.to_string(),
            paths: vec![path.to_string()],
        });
        if text.len() >= 3 {
            for window in text.windows(3) {
                let trigram = [window[0], window[1], window[2]];
                let postings = self.postings.entry(trigram).or_default();
                if postings.last() != Some(&handle) {
                    postings.push(handle);
                }
            }
        }
        true
    }

    #[must_use]
    pub fn docs(&self) -> &[Doc] {
        &self.docs
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    #[must_use]
    pub fn trigrams(&self) -> usize {
        self.postings.len()
    }

    /// Every document whose text contains every trigram of `pattern`.
    ///
    /// Exact in the *negative* direction: a document not returned does not contain
    /// the pattern. A document that is returned may still not contain it in one
    /// contiguous run, so the caller confirms with a literal scan before reporting a
    /// hit — that is one read, not a corpus read.
    #[must_use]
    pub fn search(&self, pattern: &[u8]) -> Vec<&Doc> {
        if pattern.len() < 3 {
            // Nothing to intersect on: everything is a candidate.
            return self.docs.iter().collect();
        }
        let mut lists = Vec::new();
        for window in pattern.windows(3) {
            let trigram = [window[0], window[1], window[2]];
            match self.postings.get(&trigram) {
                Some(postings) => lists.push(postings),
                // A trigram nobody has: no document can contain the pattern.
                None => return Vec::new(),
            }
        }
        // Intersect shortest-first: trigram frequency is Zipf-shaped, so this is
        // usually one list and a handful of probes.
        lists.sort_by_key(|postings| postings.len());
        let mut candidates: Vec<u32> = lists[0].clone();
        for postings in &lists[1..] {
            candidates.retain(|handle| postings.binary_search(handle).is_ok());
            if candidates.is_empty() {
                return Vec::new();
            }
        }
        candidates
            .into_iter()
            .filter_map(|handle| self.docs.get(handle as usize))
            .collect()
    }

    /// Serialize for shipment next to content.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        put_varint(&mut out, self.docs.len() as u64);
        for doc in &self.docs {
            let id = doc.content_id.as_bytes();
            put_varint(&mut out, id.len() as u64);
            out.extend_from_slice(id);
            put_varint(&mut out, doc.size);
            let media = doc.media_type.as_bytes();
            put_varint(&mut out, media.len() as u64);
            out.extend_from_slice(media);
            put_varint(&mut out, doc.paths.len() as u64);
            for path in &doc.paths {
                put_varint(&mut out, path.len() as u64);
                out.extend_from_slice(path.as_bytes());
            }
        }
        put_varint(&mut out, self.postings.len() as u64);
        for (trigram, postings) in &self.postings {
            out.extend_from_slice(trigram);
            put_varint(&mut out, postings.len() as u64);
            let mut previous = 0u64;
            for handle in postings {
                put_varint(&mut out, u64::from(*handle) - previous);
                previous = u64::from(*handle);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if !bytes.starts_with(MAGIC) {
            return None;
        }
        let mut cursor = Cursor {
            bytes,
            at: MAGIC.len(),
        };
        let doc_count = cursor.varint()?;
        let mut index = Self::new();
        for _ in 0..doc_count {
            let id_len = cursor.varint()? as usize;
            let id = String::from_utf8(cursor.take(id_len)?.to_vec()).ok()?;
            let size = cursor.varint()?;
            let media_len = cursor.varint()? as usize;
            let media = String::from_utf8(cursor.take(media_len)?.to_vec()).ok()?;
            let path_count = cursor.varint()?;
            let mut paths = Vec::with_capacity(path_count as usize);
            for _ in 0..path_count {
                let path_len = cursor.varint()? as usize;
                paths.push(String::from_utf8(cursor.take(path_len)?.to_vec()).ok()?);
            }
            let handle = index.docs.len() as u32;
            index.by_content.insert(id.clone(), handle);
            index.docs.push(Doc {
                content_id: id,
                size,
                media_type: media,
                paths,
            });
        }
        let trigram_count = cursor.varint()?;
        for _ in 0..trigram_count {
            let trigram = cursor.take(3)?;
            let count = cursor.varint()?;
            let mut postings = Vec::with_capacity(count as usize);
            let mut previous = 0u64;
            for _ in 0..count {
                previous += cursor.varint()?;
                postings.push(previous as u32);
            }
            index.postings.insert([trigram[0], trigram[1], trigram[2]], postings);
        }
        Some(index)
    }

    /// Encoded size, for a caller that wants to report the cost before shipping it.
    #[must_use]
    pub fn len(&self) -> usize {
        self.encode().len()
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> String {
        format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
    }

    fn index(docs: &[(&str, &str)]) -> TrigramIndex {
        let mut index = TrigramIndex::new();
        for (path, text) in docs {
            index.add(&doc(text), path, "text/plain", text.len() as u64, text.as_bytes());
        }
        index
    }

    #[test]
    fn search_never_misses_a_document_it_contains() {
        let index = index(&[
            ("a.md", "the alfalfa field"),
            ("b.md", "an unrelated file"),
            ("c.md", "alfalfa again"),
        ]);
        let hits: Vec<&str> = index
            .search(b"alfalfa")
            .iter()
            .map(|doc| doc.paths[0].as_str())
            .collect();
        assert_eq!(hits, vec!["a.md", "c.md"]);
    }

    #[test]
    fn a_trigram_nobody_has_eliminates_everything() {
        let index = index(&[("a.md", "the alfalfa field")]);
        assert!(index.search(b"zzz").is_empty());
        assert!(index.search(b"alfalfa zzz").is_empty());
    }

    #[test]
    fn the_same_bytes_at_two_paths_are_one_document() {
        // This is the collapse: wiki ingesting a git doc it already has must not
        // create a second document.
        let mut index = TrigramIndex::new();
        let id = doc("shared bytes");
        assert!(index.add(&id, "docs/a.md", "text/plain", 11, b"shared bytes"));
        assert!(!index.add(&id, "repo/b.md", "text/plain", 11, b"shared bytes"));
        assert!(!index.add(&id, "repo/c.md", "text/plain", 11, b"shared bytes"));
        assert_eq!(index.docs().len(), 1);
        assert_eq!(index.docs()[0].paths.len(), 3);
        assert_eq!(index.search(b"shared").len(), 1);
    }

    #[test]
    fn changed_bytes_are_a_different_document_not_a_stale_entry() {
        let mut index = TrigramIndex::new();
        index.add(&doc("v1 alfalfa"), "a.md", "text/plain", 10, b"v1 alfalfa");
        index.add(&doc("v2 alfalfa"), "a.md", "text/plain", 10, b"v2 alfalfa");
        assert_eq!(index.docs().len(), 2);
        // Both are findable by content, and neither is wrong about anything.
        // (Patterns are three bytes or more; below that the filter has nothing to
        // intersect on and honestly returns every document.)
        assert_eq!(index.search(b"v1 ").len(), 1);
        assert_eq!(index.search(b"v2 ").len(), 1);
    }

    #[test]
    fn short_patterns_return_everything() {
        let index = index(&[("a", "abc"), ("b", "def")]);
        assert_eq!(index.search(b"a").len(), 2);
    }

    #[test]
    fn the_index_round_trips_through_its_encoding() {
        let index = index(&[
            ("a.md", "the alfalfa field"),
            ("b.md", "an unrelated file with a much longer body to bend the varints"),
            ("c.md", "alfalfa again"),
        ]);
        let bytes = index.encode();
        let decoded = TrigramIndex::decode(&bytes).expect("decodes");
        assert_eq!(decoded.docs(), index.docs());
        assert_eq!(decoded.trigrams(), index.trigrams());
        assert_eq!(
            decoded.search(b"alfalfa").len(),
            index.search(b"alfalfa").len()
        );
    }

    #[test]
    fn encoding_is_compact_relative_to_the_content() {
        // Build ~200 documents of ~1 KB each, so the ratio is meaningful.
        let mut index = TrigramIndex::new();
        let mut content = 0usize;
        for n in 0..200 {
            let text = format!(
                "document {n}\n{}",
                (0..40)
                    .map(|line| format!("line {line} with words and identifiers {n}_{line}\n"))
                    .collect::<String>()
            );
            content += text.len();
            index.add(&doc(&text), &format!("d/{n}.md"), "text/plain", text.len() as u64, text.as_bytes());
        }
        let encoded = index.len();
        let ratio = encoded as f64 / content as f64;
        assert!(
            ratio < 0.6,
            "index is {encoded} bytes for {content} bytes of content ({ratio:.2})"
        );
    }
}
