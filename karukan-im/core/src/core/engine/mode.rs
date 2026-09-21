//! Mode switching (katakana, alphabet, live conversion)

use karukan_engine::width::{to_full_width, to_half_width};
use tracing::debug;

use super::*;

/// The Latin forms Ctrl+L walks, in order: what was typed, then the case
/// and width variants — mozc's F10 half-width set followed by its F9
/// full-width one. A form that repeats an earlier one (`123` upper-cased
/// is still `123`) drops out, so every press lands somewhere new and a
/// walk over digits is one stop long.
fn alphabet_forms(origin: &str) -> Vec<(String, &'static str)> {
    let half = to_half_width(origin);
    let upper = half.to_ascii_uppercase();
    let mut chars = half.chars();
    let capitalized = match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    };

    let mut forms: Vec<(String, &'static str)> = Vec::new();
    for (text, label) in [
        (half.clone(), "[半]英字"),
        (upper.clone(), "[半]英大文字"),
        (capitalized, "[半]先頭大文字"),
        (to_full_width(&half), "[全]英字"),
        (to_full_width(&upper), "[全]英大文字"),
    ] {
        if !forms.iter().any(|(seen, _)| *seen == text) {
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

    /// Ctrl+L: put back what was typed, as Latin text.
    ///
    /// The composition is replaced by its own keystrokes (`ぷろぐらむ` →
    /// `puroguramu`) and kana input carries straight on, so the Japanese
    /// after the Latin word costs no mode key: the converted text is
    /// settled, and the keystrokes that follow romanize as usual
    /// (`hello` + `ka` → 「helloか」). Any temporary mode the press found
    /// itself in ends here, which is what brings Shift+letter's Alphabet
    /// back to kana; a deliberate Katakana mode is left alone.
    ///
    /// Pressing again walks the case and width forms in
    /// [`alphabet_forms`] — the walk is keyed on the text, not the mode,
    /// so it survives the switch. Pressing after anything else starts
    /// over from the current typing. To keep typing *Latin* after the
    /// conversion, Shift+letter opens direct input as it always does.
    ///
    /// Works from Conversion too: the composition is untouched while
    /// candidates are up, so the keystrokes are still there to hand back.
    pub(super) fn convert_to_alphabet(&mut self) -> EngineResult {
        if self.input_buf.is_empty() {
            return EngineResult::not_consumed();
        }

        // A press that follows its own output continues the walk.
        let continuing = self
            .alphabet_cycle
            .as_ref()
            .filter(|cycle| cycle.produced == self.input_buf.display());
        let (origin, step) = match continuing {
            Some(cycle) => (cycle.origin.clone(), cycle.index + 1),
            None => (self.input_buf.raw(), 0),
        };

        let forms = alphabet_forms(&origin);
        if forms.is_empty() {
            // Nothing was typed that has a Latin form (an empty origin).
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
        debug!("Ctrl+L: {} → {} ({})", origin, text, label);

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
        });

        let preedit = self.set_composing_state();
        let aux = format!("{} 英数変換: {}", self.mode_indicator(), label);
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
