//! Latin words typed inline, and the split that reads them back.
//!
//! Romaji input has no way to say "this run of letters is English". Typing
//! `closesite` romanizes to 「cぉせして」 — the `c` has no rule and passes
//! through, `lo` fires the small-vowel escape — and the reading it makes is
//! not a reading of anything. The keystrokes still are: `close` is a word
//! the dictionary knows and `site` is して.
//!
//! [`split_latin_mixed`] finds that reading. It is a *derived* view of the
//! keystrokes, recomputed from scratch on each one, so the buffer the user
//! is editing never changes underneath them: Backspace still removes the
//! `e` of `close`, not the 「て」 it helped make.
//!
//! The word list comes from the system dictionary's own latin surfaces
//! ([`LatinWords`]) — the words that already answer a katakana reading, so
//! `close`, `rust` and `github` are in it and no new data file is.

use std::collections::HashSet;

use crate::dict::Dictionary;
use crate::romaji::RomajiConverter;

/// Shortest run the word list will accept. Two letters is as short as the
/// boundary can usefully be (`ai`, `pc`); one would match everywhere.
pub const MIN_LATIN_WORD_CHARS: usize = 2;

/// Longest run considered. Nothing in the list is longer, and the cap
/// bounds the scan over a long composition.
pub const MAX_LATIN_WORD_CHARS: usize = 24;

/// How many words one composition may be cut into. A second word covers
/// 「rustでcloseして」; past that the split is guesswork, and each level
/// multiplies the scan.
const MAX_LATIN_SPANS: usize = 3;

/// Whether `text` holds a half-width latin letter.
pub fn has_latin(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphabetic())
}

/// The latin words the loaded dictionaries know, lowercased.
///
/// Only the surfaces that are *entirely* latin letters are words: a mixed
/// surface like 「EPICを」 is a phrase, and matching it against keystrokes
/// would cut the particle off the reading it belongs to.
#[derive(Debug, Default)]
pub struct LatinWords {
    words: HashSet<Box<str>>,
}

impl LatinWords {
    /// An empty list — every lookup misses, so the split never fires.
    /// What the engine runs with until a dictionary is loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Collect every all-latin surface in `dict`. One pass over the
    /// entries; on the shipped dictionary that is ~2M entries and ~64k
    /// words, well under a second.
    pub fn add_dictionary(&mut self, dict: &Dictionary) {
        for surface in dict.surfaces() {
            let len = surface.chars().count();
            if !(MIN_LATIN_WORD_CHARS..=MAX_LATIN_WORD_CHARS).contains(&len) {
                continue;
            }
            if !surface.chars().all(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            self.words
                .insert(surface.to_ascii_lowercase().into_boxed_str());
        }
    }

    /// Whether `word` is in the list, ignoring case.
    pub fn contains(&self, word: &str) -> bool {
        self.words.contains(word.to_ascii_lowercase().as_str())
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Whether `surface` spells something in latin that `reading` never had.
///
/// Compared run by run, ignoring case. 「Firebaseぷろじぇくと」 →
/// 「Firebaseプロジェクト」 keeps its `Firebase`, because the reading was
/// typed with it; 「えぴっくyは」 → 「EPICyは」 does not keep its `EPIC`,
/// because the only latin the reading holds is the stray `y` of a typo.
/// A surface with no latin in it, or none the reading is missing, is not
/// an alphabet spelling of that reading at all.
pub fn has_latin_beyond(reading: &str, surface: &str) -> bool {
    let reading = reading.to_ascii_lowercase();
    let mut run = String::new();
    let mut beyond = false;
    for ch in surface.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphabetic() {
            run.push(ch.to_ascii_lowercase());
            continue;
        }
        if !run.is_empty() {
            beyond |= !reading.contains(run.as_str());
            run.clear();
        }
        if beyond {
            return true;
        }
    }
    beyond
}

/// `raw` as kana, or `None` when a keystroke passed through unconverted.
///
/// The converter's contract is that no rule ever outputs ASCII, so an
/// ASCII letter in the result is a keystroke no rule reached — exactly the
/// junk that says this run was not romaji.
fn as_kana(romaji: &RomajiConverter, raw: &str) -> Option<String> {
    if raw.is_empty() {
        return Some(String::new());
    }
    let text = romaji.convert_flush(raw);
    (!has_latin(&text)).then_some(text)
}

/// Read `raw` as kana with any latin words in it kept as they were typed.
///
/// `None` when the keystrokes romanize cleanly (nothing to reinterpret) or
/// when no split explains them — the caller then keeps the plain reading.
///
/// The scan takes the **longest** word first, which is what picks `close`
/// out of `closesite` rather than the `cl` and `clos` that are also in the
/// list, and `github` out of `githubwo` rather than `git`. A word only
/// counts if everything around it is kana, so a split that leaves junk
/// behind is no split at all.
pub fn split_latin_mixed(
    raw: &str,
    romaji: &RomajiConverter,
    words: &LatinWords,
) -> Option<String> {
    if words.is_empty() || !has_latin(&romaji.convert_flush(raw)) {
        return None;
    }
    let split = split_spans(raw, romaji, words, MAX_LATIN_SPANS)?;
    (split != raw).then_some(split)
}

/// One level of [`split_latin_mixed`]: cut one word out of `raw`, then
/// read what follows it the same way.
fn split_spans(
    raw: &str,
    romaji: &RomajiConverter,
    words: &LatinWords,
    budget: usize,
) -> Option<String> {
    if let Some(kana) = as_kana(romaji, raw) {
        return Some(kana);
    }
    if budget == 0 {
        return None;
    }

    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    for len in (MIN_LATIN_WORD_CHARS..=n.min(MAX_LATIN_WORD_CHARS)).rev() {
        for start in 0..=(n - len) {
            let span = &chars[start..start + len];
            if !span.iter().all(char::is_ascii_alphabetic) {
                continue;
            }
            let word: String = span.iter().collect();
            if !words.contains(&word) {
                continue;
            }
            let head: String = chars[..start].iter().collect();
            let Some(head_kana) = as_kana(romaji, &head) else {
                continue;
            };
            let tail: String = chars[start + len..].iter().collect();
            let Some(tail_text) = split_spans(&tail, romaji, words, budget - 1) else {
                continue;
            };
            return Some(format!("{head_kana}{word}{tail_text}"));
        }
    }
    None
}

/// Read `raw` as an alphabet-mode word followed by kana: 「EPICha」 →
/// 「EPICは」.
///
/// Alphabet mode settles its keystrokes as typed, so there is no junk to
/// give the boundary away — the *case change* is the boundary instead.
/// The rule is deliberately narrow, because the mode's whole job is to
/// type latin literally and a wrong split would fight that:
///
/// - an upper-case run of at least two letters, which the word list knows
/// - then only lower-case letters, which must make clean kana
/// - and no `l` or `x` among them: those are the small-kana escapes, and
///   an English tail leans on them (`Hello` → `H` + 「えっぉ」), so barring
///   them keeps ordinary latin words whole
pub fn split_uppercase_mixed(
    raw: &str,
    romaji: &RomajiConverter,
    words: &LatinWords,
) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let chars: Vec<char> = raw.chars().collect();
    let head_len = chars.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if head_len < MIN_LATIN_WORD_CHARS || head_len == chars.len() {
        return None;
    }
    let tail = &chars[head_len..];
    if !tail
        .iter()
        .all(|c| c.is_ascii_lowercase() && !matches!(c, 'l' | 'x'))
    {
        return None;
    }
    let word: String = chars[..head_len].iter().collect();
    if !words.contains(&word) {
        return None;
    }
    let tail_kana = as_kana(romaji, &tail.iter().collect::<String>())?;
    (!tail_kana.is_empty()).then(|| format!("{word}{tail_kana}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> LatinWords {
        LatinWords {
            words: list.iter().map(|w| (*w).into()).collect(),
        }
    }

    fn split(raw: &str, list: &[&str]) -> Option<String> {
        split_latin_mixed(raw, &RomajiConverter::new(), &words(list))
    }

    #[test]
    fn splits_a_word_off_the_kana_after_it() {
        // `closesite` romanizes to 「cぉせして」; `close` is the word and
        // `site` is the reading that follows it.
        assert_eq!(
            split("closesite", &["close", "cl", "clos", "site"]).as_deref(),
            Some("closeして")
        );
    }

    #[test]
    fn takes_the_longest_word_not_the_first_prefix() {
        // `cl` and `clos` are real dictionary surfaces and both leave a
        // convertible tail, so only longest-first picks `close`.
        assert_eq!(
            split("closesite", &["cl", "clos", "close"]).as_deref(),
            Some("closeして")
        );
        assert_eq!(
            split("githubwo", &["git", "github"]).as_deref(),
            Some("githubを")
        );
    }

    #[test]
    fn keeps_the_case_that_was_typed() {
        assert_eq!(split("GitHubwo", &["github"]).as_deref(), Some("GitHubを"));
    }

    #[test]
    fn clean_romaji_is_left_alone() {
        // 「すし」 is a reading, not a word typed in latin, even though
        // `sushi` is a surface the dictionary knows.
        assert_eq!(split("sushi", &["sushi"]), None);
        assert_eq!(split("kuro-zusite", &["close"]), None);
    }

    #[test]
    fn a_split_that_leaves_junk_is_no_split() {
        // Nothing in the list explains the `c`, so the keystrokes keep
        // their plain reading.
        assert_eq!(split("closesite", &["site"]), None);
        assert_eq!(split("closesite", &[]), None);
    }

    #[test]
    fn reads_a_second_word_after_the_first() {
        assert_eq!(
            split("rustdeclosesite", &["rust", "close"]).as_deref(),
            Some("rustでcloseして")
        );
    }

    #[test]
    fn latin_the_reading_never_had() {
        // The alphabet spelling of a kana reading.
        assert!(has_latin_beyond("くろーずして", "closeして"));
        assert!(has_latin_beyond("えぴっく", "EPIC"));
        // Latin the reading was typed with rides along.
        assert!(!has_latin_beyond(
            "Firebaseぷろじぇくと",
            "Firebaseプロジェクト"
        ));
        // A reading with no kana in it is alphabet typing outright; that
        // is the caller's gate, not this one's, so a raw-keystroke
        // shortcut reads as "beyond" here and is kept there.
        assert!(has_latin_beyond("m", "masahiro"));
        // One stray keystroke does not license a whole word.
        assert!(has_latin_beyond("えぴっくyは", "EPICyは"));
        // Nothing latin to judge.
        assert!(!has_latin_beyond("えぴっく", "エピック"));
    }

    #[test]
    fn uppercase_run_splits_on_the_case_change() {
        let w = words(&["epic"]);
        let r = RomajiConverter::new();
        assert_eq!(
            split_uppercase_mixed("EPICha", &r, &w).as_deref(),
            Some("EPICは")
        );
        assert_eq!(
            split_uppercase_mixed("EPICwo", &r, &w).as_deref(),
            Some("EPICを")
        );
    }

    #[test]
    fn uppercase_split_leaves_ordinary_latin_alone() {
        let w = words(&["epic", "hello", "he", "macbook", "mac"]);
        let r = RomajiConverter::new();
        // No tail at all.
        assert_eq!(split_uppercase_mixed("EPIC", &r, &w), None);
        // `ello` leans on the `l` escape — an English word, not a reading.
        assert_eq!(split_uppercase_mixed("Hello", &r, &w), None);
        // A second upper-case letter in the tail means camel case.
        assert_eq!(split_uppercase_mixed("MacBook", &r, &w), None);
        // A head the dictionary has never seen.
        assert_eq!(split_uppercase_mixed("QQQha", &r, &w), None);
    }
}
