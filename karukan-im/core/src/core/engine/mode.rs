//! Mode switching (katakana, alphabet, live conversion)

use karukan_engine::kana::{hiragana_to_katakana, katakana_to_half_width};
use karukan_engine::width::{to_full_width, to_half_width};
use tracing::debug;

use super::*;

/// Which walk a press is on. One key, one set of forms: a press only ever
/// moves within its own, and a press on a different key restarts from the
/// top of that key's set — the way ATOK's F8/F9/F10 hand the same reading
/// to one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::core) enum FormSet {
    /// Ctrl+L: every Latin form, half-width first. karukan's own key,
    /// covering ATOK's F10 and F9 in one walk.
    Latin,
    /// Ctrl+O — ATOK's F8 「半角変換」: half-width katakana, then the
    /// half-width Latin forms.
    Half,
    /// Ctrl+P — ATOK's F9 「全角英字変換」: the full-width Latin forms,
    /// lowercase → uppercase → initial capital.
    FullLatin,
}

impl FormSet {
    /// What the aux line calls this walk.
    fn name(self) -> &'static str {
        match self {
            FormSet::Latin => "英数変換",
            FormSet::Half => "半角変換",
            FormSet::FullLatin => "全角英字変換",
        }
    }
}

/// The forms one key walks, in order. `origin` is the keystrokes every
/// Latin form is cut from; `reading` is the kana they make, which is what
/// half-width katakana has to come from. A form that repeats an earlier
/// one (`123` upper-cased is still `123`) drops out, so every press lands
/// somewhere new and a walk over digits is one stop long.
fn character_forms(origin: &str, reading: &str, set: FormSet) -> Vec<(String, &'static str)> {
    let half = to_half_width(origin);
    let upper = half.to_ascii_uppercase();
    let mut chars = half.chars();
    let capitalized = match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    };

    let table: Vec<(String, &'static str)> = match set {
        FormSet::Latin => vec![
            (half.clone(), "[半]英字"),
            (upper.clone(), "[半]英大文字"),
            (capitalized, "[半]先頭大文字"),
            (to_full_width(&half), "[全]英字"),
            (to_full_width(&upper), "[全]英大文字"),
        ],
        FormSet::Half => vec![
            (
                katakana_to_half_width(&hiragana_to_katakana(reading)),
                "[半]カタカナ",
            ),
            (half.clone(), "[半]英字"),
            (upper.clone(), "[半]英大文字"),
            (capitalized, "[半]先頭大文字"),
        ],
        FormSet::FullLatin => vec![
            (to_full_width(&half), "[全]英字"),
            (to_full_width(&upper), "[全]英大文字"),
            (to_full_width(&capitalized), "[全]先頭大文字"),
        ],
    };

    let mut forms: Vec<(String, &'static str)> = Vec::new();
    for (text, label) in table {
        if !text.is_empty() && !forms.iter().any(|(seen, _)| *seen == text) {
            forms.push((text, label));
        }
    }
    forms
}

impl InputMethodEngine {
    /// Enter katakana mode (Ctrl+k)
    /// One-way switch to Katakana; a mode toggle key (Right Super, JIS 変換,
    /// macOS かな/right-⌘ tap) returns to Hiragana.
    pub(super) fn enter_katakana_mode(&mut self) -> EngineResult {
        // Already in katakana mode: nothing to do
        if self.mode.current() == InputMode::Katakana {
            return EngineResult::consumed();
        }

        self.mode.set(InputMode::Katakana);
        // Drop the live display so katakana mode takes priority on commit
        self.live.shown = false;

        if self.input_buf.is_empty() {
            return EngineResult::consumed();
        }

        let preedit = self.set_composing_state();

        // Update aux text to show mode
        let aux = format!("{} Karukan ({})", self.mode_indicator(), self.model_name());

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::UpdateAuxText(aux))
    }

    /// Toggle live conversion mode via Ctrl+Shift+L.
    ///
    /// When toggled ON during Composing, immediately convert the current
    /// input buffer so the user doesn't have to type another key to see the
    /// live result. When toggled OFF, drop any stale converted text so the
    /// preedit reverts to hiragana right away.
    pub(super) fn toggle_live_conversion(&mut self) -> EngineResult {
        self.live.enabled = !self.live.enabled;
        let mode = if self.live.enabled { "ON" } else { "OFF" };
        debug!("Live conversion toggled: {}", mode);
        let aux = EngineAction::UpdateAuxText(format!("ライブ変換: {}", mode));

        if matches!(self.state, InputState::Composing { .. })
            && self.mode.current() != InputMode::Katakana
        {
            if self.live.enabled {
                let mut result = self.refresh_input_state();
                result.actions.push(aux);
                return result;
            }
            if self.live.shown {
                self.live.shown = false;
                let preedit = self.set_composing_state();
                return EngineResult::consumed()
                    .with_action(EngineAction::UpdatePreedit(preedit))
                    .with_action(aux);
            }
        }

        EngineResult::consumed().with_action(aux)
    }

    /// Ctrl+L / Ctrl+O / Ctrl+P: put back what was typed, in the form
    /// `set` names.
    ///
    /// The composition is replaced by one of its own forms (`ぷろぐらむ` →
    /// `puroguramu`) and kana input carries straight on, so the Japanese
    /// after the Latin word costs no mode key: the converted text is
    /// settled, and the keystrokes that follow romanize as usual
    /// (`hello` + `ka` → 「helloか」). Any temporary mode the press found
    /// itself in ends here, which is what brings Shift+letter's Alphabet
    /// back to kana; a deliberate Katakana mode is left alone.
    ///
    /// Pressing the same key again walks that set's forms — the walk is
    /// keyed on the text, not the mode, so it survives the switch.
    /// Pressing a *different* one of these keys re-cuts the same original
    /// keystrokes into its own set, so 半角 and 全角英字 hand the reading
    /// back and forth without a retype. Pressing after anything else
    /// starts over from the current typing. To keep typing *Latin* after
    /// the conversion, Shift+letter opens direct input as it always does.
    ///
    /// Works from Conversion too: the composition is untouched while
    /// candidates are up, so the keystrokes are still there to hand back.
    pub(super) fn convert_to_form(&mut self, set: FormSet) -> EngineResult {
        if self.input_buf.is_empty() {
            return EngineResult::not_consumed();
        }

        // A press that follows one of these keys' own output keeps the
        // keystrokes they were all cut from; only the same key continues
        // the walk, a different one opens its set at the top.
        let standing = self
            .alphabet_cycle
            .as_ref()
            .filter(|cycle| cycle.produced == self.input_buf.display());
        let (origin, step) = match standing {
            Some(cycle) if cycle.set == set => (cycle.origin.clone(), cycle.index + 1),
            Some(cycle) => (cycle.origin.clone(), 0),
            None => (self.input_buf.raw(), 0),
        };

        let reading = self.converters.romaji.convert_flush(&origin);
        let forms = character_forms(&origin, &reading, set);
        if forms.is_empty() {
            // Nothing was typed that has a form in this set (an empty
            // origin, or kana-only input asked for its Latin forms).
            return EngineResult::consumed();
        }
        // A press must always change something. Starting a walk on text
        // that is already its own first form (Latin typed in alphabet
        // mode) would look dead, so that press takes the next one.
        let step = if step == 0 && forms[0].0 == self.input_buf.display() {
            1
        } else {
            step
        };
        let (text, label) = forms[step % forms.len()].clone();
        debug!("{}: {} → {} ({})", set.name(), origin, text, label);

        // The live display and the chunks were built from a reading that
        // no longer exists, so the whole composition-scoped set goes.
        self.clear_composition();
        for ch in text.chars() {
            self.input_buf.push_direct(ch);
        }
        // Back to kana for whatever comes next. `exit_temporary` is the
        // whole switch: it drops Shift+letter's Alphabet (and Emoji) and
        // leaves a mode the user picked outright — Katakana — in place.
        self.mode.exit_temporary();
        self.alphabet_cycle = Some(AlphabetCycle {
            origin,
            produced: text,
            index: step % forms.len(),
            set,
        });

        let preedit = self.set_composing_state();
        let aux = format!("{} {}: {}", self.mode_indicator(), set.name(), label);
        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::UpdateAuxText(aux))
    }

    /// Ctrl+Shift+V: turn the aux line's debug details on or off. The next
    /// render picks it up, so no state has to be rebuilt here. The key only
    /// reaches this while something is being typed (the Empty state passes
    /// the chord through to the application, whose paste it usually is).
    pub(super) fn toggle_verbose(&mut self) -> EngineResult {
        self.config.verbose = !self.config.verbose;
        let mode = if self.config.verbose { "ON" } else { "OFF" };
        debug!("Verbose display toggled: {}", mode);
        // Re-render the line the user is looking at, so the change shows now
        // rather than on the next keystroke. The Empty arm is unreachable from
        // the key binding; it reports the toggle for any other caller.
        let aux = match &self.state {
            InputState::Conversion {
                reading,
                candidates,
                ..
            } => self.format_aux_conversion(reading, candidates),
            InputState::Composing { .. } => self.format_aux_suggest(),
            InputState::Empty => format!("詳細表示: {mode}"),
        };
        EngineResult::consumed().with_action(EngineAction::UpdateAuxText(aux))
    }
}
