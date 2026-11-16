//! Which installed Spore opens this path, and which one the person asked for.
//!
//! Two facts meet here, and they have different owners:
//!
//! - **What a Spore can open** is the Spore's own declaration, the `handles` list
//!   in its manifest. A program's reach is a property of the program.
//! - **What should open it** is a preference, so it lives in a record that syncs
//!   with the desktop rather than in one machine's dotfiles.
//!
//! The rule that keeps them honest: a preference may only *choose among the
//! declarations*. A default naming a Spore that no longer handles the kind is
//! ignored, not obeyed — otherwise a stale preference becomes a capability claim.

use std::collections::BTreeMap;

/// What the host knows about a path before opening it.
///
/// Both are optional because a host may have neither: an extension it does not
/// recognise, a file with no suffix, or a store that exposes no media type. The
/// absence is real, and [`candidates`] answers it with the truth (nothing) rather
/// than guessing from the name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kinds {
    /// Sniffed or declared media type, e.g.
    /// `application/vnd.openxmlformats-officedocument.wordprocessingml.document`.
    pub media_type: Option<String>,
    /// The extension without the dot, lowercased: `docx`.
    pub extension: Option<String>,
}

impl Kinds {
    pub fn from_path(path: &str, media_type: Option<&str>) -> Self {
        let extension = path
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .filter(|extension| !extension.is_empty() && !extension.contains('/'));
        Self {
            media_type: media_type
                .map(str::trim)
                .filter(|kind| !kind.is_empty())
                .map(|kind| kind.to_ascii_lowercase()),
            extension,
        }
    }
}

/// One installed Spore, as the catalog reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    /// The `handles` list from its manifest, exactly as declared.
    pub handles: Vec<String>,
}

/// A Spore that can open the path, and the declaration that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// The declaration that matched, so a menu can say *why* it is offered.
    pub handles: String,
    /// True for a media-type match, false for an extension glob.
    pub exact: bool,
}

/// Everything that can open this path, most specific first.
///
/// A media-type match outranks an extension match: the type is what the bytes
/// are, the extension is what the name claims. Ties fall back to the Spore id so
/// the order is stable across runs and machines.
pub fn candidates(kinds: &Kinds, installed: &[Installed]) -> Vec<Candidate> {
    let mut found = Vec::new();
    for spore in installed {
        let mut best: Option<(String, bool)> = None;
        for handle in &spore.handles {
            let handle = handle.trim();
            if let Some(media_type) = &kinds.media_type
                && handle.eq_ignore_ascii_case(media_type)
                && best.as_ref().is_none_or(|(_, exact)| !exact)
            {
                best = Some((handle.to_string(), true));
            }
            if let Some(extension) = &kinds.extension
                && let Some(suffix) = handle.strip_prefix("*.")
                && suffix.eq_ignore_ascii_case(extension)
                && best.is_none()
            {
                best = Some((handle.to_string(), false));
            }
        }
        if let Some((handles, exact)) = best {
            found.push(Candidate {
                id: spore.id.clone(),
                name: spore.name.clone(),
                handles,
                exact,
            });
        }
    }
    found.sort_by(|left, right| {
        right
            .exact
            .cmp(&left.exact)
            .then(left.id.cmp(&right.id))
    });
    found
}

/// The Spore that should open this path.
///
/// `defaults` maps a kind (media type or extension glob, as declared) to a Spore
/// id. The most specific key wins: a media type beats a glob, because the person
/// who set "Word documents open in doc-shell" meant the type and not the four
/// letters. A key naming something that is not among `candidates` is skipped —
/// a preference can choose among declarations, never create one.
pub fn choose<'a>(
    kinds: &Kinds,
    candidates: &'a [Candidate],
    defaults: &BTreeMap<String, String>,
) -> Option<&'a Candidate> {
    let media_type = kinds
        .media_type
        .as_ref()
        .and_then(|media_type| defaults.get(media_type));
    let extension = kinds
        .extension
        .as_ref()
        .and_then(|extension| defaults.get(&format!("*.{extension}")));
    for wanted in [media_type, extension].into_iter().flatten() {
        if let Some(candidate) = candidates.iter().find(|candidate| &candidate.id == wanted) {
            return Some(candidate);
        }
    }
    candidates.first()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_shell() -> Installed {
        Installed {
            id: "dev.fungos.doc-shell".into(),
            name: "doc-shell".into(),
            handles: vec![
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .into(),
                "text/plain".into(),
                "*.docx".into(),
                "*.txt".into(),
            ],
        }
    }

    fn editor() -> Installed {
        Installed {
            id: "dev.fungos.editor".into(),
            name: "editor".into(),
            handles: vec!["text/plain".into(), "*.txt".into()],
        }
    }

    #[test]
    fn a_media_type_outranks_an_extension() {
        let mut vague = doc_shell();
        vague.id = "a.vague".into();
        vague.handles = vec!["*.txt".into()];
        let kinds = Kinds::from_path("notes.txt", Some("text/plain"));
        let found = candidates(&kinds, &[vague, editor()]);
        assert_eq!(found.len(), 2);
        assert!(found[0].exact, "the type is what the bytes are");
        assert_eq!(found[0].id, "dev.fungos.editor");
        assert!(!found[1].exact);
    }

    #[test]
    fn the_extension_answers_when_the_type_does_not() {
        let kinds = Kinds::from_path("brief.DOCX", None);
        assert_eq!(kinds.extension.as_deref(), Some("docx"), "lowercased");
        let found = candidates(&kinds, &[doc_shell(), editor()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].handles, "*.docx");
    }

    #[test]
    fn nothing_is_an_answer() {
        let kinds = Kinds::from_path("holiday.heic", Some("image/heic"));
        assert!(candidates(&kinds, &[doc_shell(), editor()]).is_empty());
        assert_eq!(Kinds::from_path("Makefile", None).extension, None);
    }

    #[test]
    fn a_preference_chooses_among_declarations_and_never_creates_one() {
        let kinds = Kinds::from_path("notes.txt", Some("text/plain"));
        let found = candidates(&kinds, &[doc_shell(), editor()]);

        let mut defaults = BTreeMap::new();
        defaults.insert("text/plain".to_string(), "dev.fungos.doc-shell".to_string());
        let chosen = choose(&kinds, &found, &defaults).expect("a choice");
        assert_eq!(chosen.id, "dev.fungos.doc-shell");

        // A default naming something that does not handle the kind is ignored:
        // a stale preference must not become a capability claim.
        defaults.insert("text/plain".to_string(), "dev.fungos.gone".to_string());
        let chosen = choose(&kinds, &found, &defaults).expect("a choice");
        assert_eq!(
            chosen.id, found[0].id,
            "a stale preference falls back to the first candidate, not to a guess"
        );
    }

    #[test]
    fn the_media_type_preference_beats_the_glob_preference() {
        let kinds = Kinds::from_path("brief.docx", Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ));
        let found = candidates(&kinds, &[doc_shell(), editor()]);
        let mut defaults = BTreeMap::new();
        defaults.insert("*.docx".to_string(), "dev.fungos.doc-shell".to_string());
        assert_eq!(
            choose(&kinds, &found, &defaults).map(|candidate| candidate.id.as_str()),
            Some("dev.fungos.doc-shell")
        );
        assert_eq!(found[0].handles, "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    }
}
