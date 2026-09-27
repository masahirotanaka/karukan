//! Reading a latin word typed inline back as itself.
//!
//! Romaji input has no way to mark a run of letters as English, so
//! `closesite` settles as 「cぉせして」: the `c` reached no rule and passed
//! through, `lo` fired the small-vowel escape. That is not a reading of
//! anything, and the keystrokes behind it still say what was meant —
//! `close` is a word the dictionary knows and `site` is して.
//!
//! The split itself is `karukan_engine::latin`, which knows nothing of the
//! engine. What lives here is when to ask it: which keystrokes to hand it
//! (the reading's, never the half-typed tail) and which of the two rules
//! the current mode calls for — the junk-boundary one in kana mode, the
//! case-boundary one in alphabet mode, where keystrokes settle as typed
//! and there is no junk to give the boundary away.
//!
//! The result is *derived*, recomputed from the keystrokes on every one of
//! them. The buffer is never rewritten, so Backspace still takes the `e`
//! of `close` rather than the 「て」 it helped make, and Ctrl+L still has
//! the original typing to hand back.

use super::*;

impl InputMethodEngine {
    /// The composing reading with any inline latin word kept as typed, or
    /// `None` when the keystrokes are ordinary romaji (the overwhelmingly
    /// common case, and the one this must stay cheap for).
    pub(super) fn latin_mixed_reading(&self) -> Option<String> {
        if !self.config.latin_input || self.dicts.latin.is_empty() {
            return None;
        }
        // An emoji query is a query, not typing: 「:smile」 is latin with
        // a passed-through `:` in front of it, which is exactly the shape
        // the split looks for, and reading it back as 「：smile」 would
        // take the picker's place.
        if self.mode.current() == InputMode::Emoji {
            return None;
        }
        let raw = self.input_buf.raw_reading();
        if raw.chars().count() < karukan_engine::latin::MIN_LATIN_WORD_CHARS {
            return None;
        }
        let split = if self.mode.current() == InputMode::Alphabet {
            // Alphabet mode settles keystrokes as typed, so the reading
            // *is* the keystrokes and the case change is the only
            // boundary there is: 「EPICha」 → 「EPICは」.
            karukan_engine::split_uppercase_mixed(&raw, &self.converters.romaji, &self.dicts.latin)
        } else {
            karukan_engine::split_latin_mixed(&raw, &self.converters.romaji, &self.dicts.latin)
        }?;
        // A split that reproduces the reading has found nothing: the
        // caller keeps what it had rather than re-running the model on an
        // identical string.
        (split != self.input_buf.reading()).then_some(split)
    }

    /// Whether `surface` is an alphabet spelling being offered for a
    /// reading that has no alphabet in it — the dictionary's
    /// 「くろーず → close」, a learned 「えぴっく → EPIC」.
    ///
    /// Those are what `[conversion] alphabet_from_kana = false` drops:
    /// alphabet is what alphabet keystrokes produce, and `latin_input` is
    /// how they are typed inline.
    ///
    /// Only latin the reading never had counts, so the two ways a reading
    /// legitimately carries it both survive: a composition that switched
    /// modes mid-word (「Firebaseぷろじぇくと」 → 「Firebaseプロジェクト」)
    /// and a reading with no kana at all, which is alphabet typing by
    /// definition — the raw-keystroke shortcut `m` keeps answering with
    /// its address. The user's own dictionary is exempt wherever it is
    /// consulted: registering an entry is declaring that this reading has
    /// that surface.
    pub(super) fn drops_alphabet_surface(&self, reading: &str, surface: &str) -> bool {
        !self.config.alphabet_from_kana
            && karukan_engine::contains_kana(reading)
            && karukan_engine::has_latin_beyond(reading, surface)
    }
}
