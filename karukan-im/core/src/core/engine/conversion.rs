//! Conversion state handling: building the mixed candidate list, key
//! handling, and commit. Model dispatch lives in the sibling `model`
//! module, the Ctrl+R source views in `filter`, live chunking in `chunk`.

use std::collections::HashSet;

use tracing::debug;

use super::filter::source_for_key;
use super::*;

/// Maximum number of learning candidates to show
const MAX_LEARNING_CANDIDATES: usize = 3;

/// Annotation on a candidate converted from a repaired reading rather
/// than the one typed.
const CORRECTION_LABEL: &str = "もしかして";

/// How much surer of the repaired reading the model must be before the
/// repair replaces what was typed instead of sitting under it, as a ratio
/// of per-character NLL.
///
/// Measured on jinen-v2-small over 19 readings: ones that really had lost
/// an `n` score 3.5–3800× worse than their repair, while correct typing
/// whose repair is nonsense stays at or under 1.5×. Three sits in that
/// gap. Erring low silently rewrites what someone typed; erring high only
/// leaves the repair where it already was, a candidate away — so when in
/// doubt this number goes up.
const AUTO_CORRECTION_RATIO: f32 = 3.0;

/// How much surer of *what was typed* the model must be before a repair
/// is dropped rather than offered. The mirror of
/// [`AUTO_CORRECTION_RATIO`]: correct typing throws off repairs that
/// score 20× worse than the real reading (にほんごのべんきょう against
/// にほんごんおべんきょう), and there is no reason to show those.
const REPAIR_NOISE_RATIO: f32 = 3.0;

/// What the model made of a repaired reading, weighed against what was
/// actually typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Far surer of the repair: it is what the typing meant.
    Replace,
    /// Too close to call, or not weighed at all — let the user choose.
    Offer,
    /// Far surer of what was typed: the repair is noise.
    Drop,
}

/// Shortest reading a repair may replace outright.
///
/// Two or three kana carry too little for the score to mean anything
/// about the typing: かに scores 7.5× worse than かんい simply because 蟹
/// is a rarer word than 簡易, and 「かに」 must not turn into 「簡易」.
/// Below this the repair still rides as a candidate, and a word this
/// short is cheap to retype anyway.
const MIN_AUTO_CORRECTION_CHARS: usize = 5;

/// How many repaired readings one conversion may try. Each is a model
/// call, so the cap is what keeps a dropped keystroke from costing a
/// conversion per `n` in the word. Two covers a word with a mistake in
/// it and the same word with the mistake somewhere else; a reading that
/// needs three tries is not a typo any more.
const MAX_CORRECTION_VARIANTS: usize = 2;

/// Max predictive (prefix-extending) dictionary candidates in the
/// composing suggestion list. The conversion list is uncapped — the full
/// ranked set goes into the paged candidate window.
const MAX_PREDICTIVE_SUGGESTIONS: usize = 3;

/// Min typed characters before predictive dictionary lookup kicks in — a
/// single key would flood the list from a large dictionary
const MIN_PREDICTIVE_PREFIX_CHARS: usize = 2;

/// How the unresolved romaji tail constrains the predictive lookup.
enum TailConstraint {
    /// No tail: prediction is unconstrained
    Unconstrained,
    /// The tail can still become these kana: narrow to them (`d` → だ/で…)
    Narrow(Vec<String>),
    /// The tail can no longer become kana (`yk`): no reading extends it
    Dead,
}

/// Mozc-style width/script annotation for a pure-kana candidate, or `None`
/// if the text mixes scripts or contains kanji/punctuation. Used to label
/// `あ` / `ア` / `ｱ` candidates in the conversion list.
pub(super) fn width_annotation(text: &str) -> Option<&'static str> {
    if karukan_engine::is_pure_hiragana(text) {
        Some("[全]ひらがな")
    } else if karukan_engine::is_pure_full_katakana(text) {
        Some("[全]カタカナ")
    } else {
        None
    }
}

/// Helper for building a deduplicated list of conversion candidates.
///
/// Two push paths exist: [`push`] dedups by text (skips duplicates), and
/// [`push_force`] always inserts (used for learning candidates that should
/// appear at the top even if a later source re-emits the same text).
struct CandidateBuilder {
    candidates: Vec<AnnotatedCandidate>,
    seen: HashSet<String>,
}

impl CandidateBuilder {
    fn new() -> Self {
        Self {
            candidates: Vec::new(),
            seen: HashSet::new(),
        }
    }

    /// Push a candidate if its text hasn't been seen yet.
    fn push(&mut self, ac: AnnotatedCandidate) {
        if self.seen.insert(ac.text.clone()) {
            self.candidates.push(ac);
        }
    }

    /// Push a candidate unconditionally, marking its text as seen so later
    /// dedup'd inserts skip it. Use only for sources that should win over
    /// duplicates from later steps (e.g. learning cache).
    fn push_force(&mut self, ac: AnnotatedCandidate) {
        self.seen.insert(ac.text.clone());
        self.candidates.push(ac);
    }

    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn into_candidates(self) -> Vec<AnnotatedCandidate> {
        self.candidates
    }
}

impl InputMethodEngine {
    /// Start kanji conversion for the current buffer (Space/Down/Tab).
    pub(super) fn start_conversion(&mut self, learning: LearningLookup) -> EngineResult {
        // Resolve the reading without touching the composition, so Esc
        // returns to an editable buffer with the romaji tail still live.
        let reading = self.input_buf.settled_reading(&self.converters.romaji);
        // The unresolved tail keeps narrowing the predictive dictionary
        // lookup (わせd → 早稲田 stays selectable).
        let base = self.input_buf.reading();
        let pending = self.input_buf.pending();

        // Snapshot the live-conversion text before clearing it, so the
        // displayed candidate survives even if re-inference diverges.
        let prev_suggest_text = self.live_text_with_pending();
        self.live.shown = false;

        if reading.is_empty() {
            return EngineResult::consumed();
        }

        // Get candidates from kanji converter (use full num_candidates for explicit conversion)
        let mut candidates = self.build_conversion_candidates(
            &reading,
            &base,
            &pending,
            self.config.num_candidates,
            learning,
        );

        let seen: HashSet<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
        if !prev_suggest_text.is_empty()
            && prev_suggest_text != reading
            && !seen.contains(prev_suggest_text.as_str())
        {
            candidates.insert(
                0,
                AnnotatedCandidate::new(prev_suggest_text, CandidateSource::Model),
            );
        }

        if candidates.is_empty() {
            // No candidates: stay composing, untouched (emoji queries with
            // no match land here)
            let preedit = self.set_composing_state();
            return EngineResult::consumed().with_action(EngineAction::UpdatePreedit(preedit));
        }

        let candidate_list = self.to_conversion_candidate_list(candidates, &reading);
        self.enter_conversion_state(&reading, candidate_list)
    }

    /// Map builder output to the public [`CandidateList`] shown in the
    /// conversion window, settled at the configured width.
    fn to_conversion_candidate_list(
        &self,
        candidates: Vec<AnnotatedCandidate>,
        reading: &str,
    ) -> CandidateList {
        self.settle_candidates(
            candidates
                .into_iter()
                .map(|ac| ac.into_candidate(reading))
                .collect(),
        )
    }

    /// Transition to Conversion state with the given reading and candidate list.
    ///
    /// Sets up the preedit (highlighted selected text), updates the state, and
    /// returns an EngineResult with preedit, candidates, and aux text actions.
    pub(super) fn enter_conversion_state(
        &mut self,
        reading: &str,
        candidates: CandidateList,
    ) -> EngineResult {
        let selected_text = candidates.selected_text().unwrap_or(reading).to_string();

        let preedit = Preedit::with_text_highlighted(&selected_text);

        self.state = InputState::Conversion {
            preedit: preedit.clone(),
            candidates: candidates.clone(),
            reading: reading.to_string(),
            // A fresh conversion always starts unfiltered
            filter: None,
        };

        // After the state assignment: the aux header reads the active filter.
        let aux = self.format_aux_conversion_with_page(reading, Some(&candidates));

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::ShowCandidates(candidates))
            .with_action(EngineAction::UpdateAuxText(aux))
    }

    /// What an exact dictionary match is looked up under, or `None` when
    /// there is nothing to match exactly.
    ///
    /// Normally the settled reading, and only while no romaji tail is
    /// pending: an exact hit on the base would ignore the tail the user is
    /// still typing, offering わせ's entries for 「わせd」.
    ///
    /// A composition that is *all* unresolved keystrokes (`m`, `gm` — a
    /// consonant run that never reached a rule) has no base to ignore, so
    /// the keystrokes are the key. That is what makes a latin shortcut
    /// reachable: a user dictionary entry read `m` answers the `m`
    /// keystroke, the way mozc looks its raw input up alongside the kana.
    /// Only exact, never predictive — a one-letter prefix over a latin
    /// dictionary would flood the window, and a shortcut is something the
    /// user typed in full.
    fn exact_match_key<'a>(reading: &'a str, pending: &'a str) -> Option<&'a str> {
        match (reading.is_empty(), pending.is_empty()) {
            (_, true) => Some(reading),
            (true, false) => Some(pending),
            (false, false) => None,
        }
    }

    /// Dictionary candidates for a reading: user dict first, then system,
    /// exact matches then predictive (prefix-extending) ones, deduped.
    ///
    /// `pending` narrows the predictive lookup to readings the romaji tail
    /// can still become (わせ + `d` keeps わせだ…, drops わせり…);
    /// `predictive_limit` caps those results. `only` restricts search and
    /// dedup to one dictionary, so shared surfaces stay visible per view.
    ///
    /// A composition that is *nothing but* an unresolved tail is looked up
    /// under the keystrokes themselves — see [`Self::exact_match_key`].
    pub(super) fn search_dictionaries(
        &self,
        reading: &str,
        pending: &str,
        limit: usize,
        predictive_limit: usize,
        min_prefix_chars: usize,
        only: Option<CandidateSource>,
    ) -> Vec<AnnotatedCandidate> {
        let dicts = [
            (self.dicts.user.as_ref(), CandidateSource::UserDictionary),
            (self.dicts.system.as_ref(), CandidateSource::Dictionary),
        ]
        .into_iter()
        .filter(|(_, source)| only.is_none_or(|o| o == *source))
        .collect::<Vec<_>>();
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        // Exact matches, user dictionary first. Candidates are sorted by
        // score at build/load time
        let exact_key = Self::exact_match_key(reading, pending);
        for &(dict, source) in &dicts {
            let Some(key) = exact_key else {
                break;
            };
            let Some(result) = dict.and_then(|d| d.exact_match_search(key)) else {
                continue;
            };
            for cand in result.candidates {
                if candidates.len() >= limit {
                    break;
                }
                if seen.insert(cand.surface.clone()) {
                    candidates.push(AnnotatedCandidate::new(cand.surface.clone(), source));
                }
            }
        }

        // Predictive: dictionary readings extending the typed prefix,
        // mirroring the learning cache's prefix lookup. The full reading
        // rides on the candidate so selecting it commits and records under
        // the right key.
        let constraint = self.tail_constraint(pending);
        if reading.chars().count() >= min_prefix_chars
            && !matches!(constraint, TailConstraint::Dead)
        {
            let mut budget = predictive_limit;
            for &(dict, source) in &dicts {
                if budget == 0 {
                    break;
                }
                let Some(dict) = dict else { continue };
                let matches = match &constraint {
                    TailConstraint::Unconstrained => dict.predictive_search(reading, budget),
                    TailConstraint::Narrow(expansions) => {
                        dict.predictive_search_expanded(reading, expansions, budget)
                    }
                    TailConstraint::Dead => unreachable!("checked above"),
                };
                for m in matches {
                    if budget == 0 || candidates.len() >= limit {
                        break;
                    }
                    if seen.insert(m.candidate.surface.clone()) {
                        budget -= 1;
                        candidates.push(
                            AnnotatedCandidate::new(m.candidate.surface.clone(), source)
                                .with_reading(Some(m.reading.to_string())),
                        );
                    }
                }
            }
        }

        candidates
    }

    /// Readings the typing might have meant, had one `n` not been
    /// dropped — `konnichiha` (こんいちは) also asks about `konnnichiha`
    /// (こんにちは).
    ///
    /// Doubling an `n` is the whole repair. `ん` is the one kana that can
    /// cost a keystroke more than it looks like it should, so the
    /// keystroke people lose is an `n`, and putting one back covers every
    /// shape the loss takes — before a vowel (`kanni` → かんい), before
    /// な行 (`konnni` → こんに), before や行 (`shinnya` → しんや). Working
    /// on the keystrokes instead of the kana is what makes it one rule
    /// rather than a table of kana pairs; the buffer already keeps them
    /// for Ctrl+L.
    ///
    /// A repair that converts to what was typed anyway is dropped, so a
    /// word with nothing wrong with it costs nothing.
    fn n_corrected_readings(&self, reading: &str) -> Vec<String> {
        let raw = self.input_buf.raw();
        if !raw.contains('n') || self.mode.current() == InputMode::Emoji {
            return Vec::new();
        }
        // The repairs have to be repairs of *this* reading. While a romaji
        // tail is still live the keystrokes say more than the reading does
        // — `sinyabasunin` reads しにゃばすに with an `n` still in hand —
        // and a repair cut from them would be weighed against a conversion
        // of something else.
        if self.converters.romaji.convert_flush(&raw) != reading {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for (at, _) in raw.char_indices().filter(|(_, c)| *c == 'n') {
            let variant = format!("{}n{}", &raw[..at], &raw[at..]);
            if !seen.insert(variant.clone()) {
                continue;
            }
            let corrected = self.converters.romaji.convert_flush(&variant);
            // No Japanese word opens with ん, so a repair that produces one
            // put the keystroke somewhere nobody typed it (`nekoga…` →
            // んえこが…). Dropping them here is what keeps the score
            // honest: 「んエコが好きです」 is the one reading the model
            // liked better than 「猫が好きです」.
            if corrected.starts_with('ん') || corrected == reading || out.contains(&corrected) {
                continue;
            }
            out.push(corrected);
            if out.len() >= MAX_CORRECTION_VARIANTS {
                break;
            }
        }
        out
    }

    /// Repairs for a dropped `n`, converted and weighed against what was
    /// actually typed.
    ///
    /// Only the first is weighed. Each weighing costs two scoring passes,
    /// and a reading whose *second* repair is the right one is not the
    /// single slipped keystroke this is for — the rest are offered as
    /// they are.
    ///
    /// Both sides are weighed the same way: the per-character NLL of the
    /// model's own answer to that reading. So this compares two
    /// conversions the model produced, never a conversion against a
    /// guess. The gate is a ratio, not a difference — per-character NLL
    /// spans orders of magnitude between a confident reading and one the
    /// model was forced through, and the two populations separate on that
    /// scale (see [`AUTO_CORRECTION_RATIO`]).
    fn weighed_repairs(&mut self, reading: &str, typed: &[String]) -> Vec<(String, Verdict)> {
        let mut out = Vec::new();
        for (nth, corrected) in self.n_corrected_readings(reading).into_iter().enumerate() {
            let converted = self.model_candidates(&corrected, 1).into_iter().next();
            // No model, or nothing came back: the repaired kana is still
            // worth offering — it is what they meant to type.
            let text = converted.unwrap_or_else(|| corrected.clone());
            let verdict = if nth == 0 {
                self.weigh(reading, typed.first(), &corrected, &text)
            } else {
                Verdict::Offer
            };
            if verdict != Verdict::Drop {
                out.push((text, verdict));
            }
        }
        out
    }

    /// The repair that should take over the live display, if any.
    ///
    /// The live path's slice of [`Self::weighed_repairs`]: only a
    /// `Replace` counts here, since an offer belongs in a candidate list
    /// someone is reading, not in text still being typed. This runs on
    /// every keystroke, so the cheap gates come before the conversion
    /// that would otherwise be paid to find out they failed.
    pub(super) fn live_replacement(&mut self, reading: &str, typed: &str) -> Option<String> {
        if !self.config.auto_correct_n || reading.chars().count() < MIN_AUTO_CORRECTION_CHARS {
            return None;
        }
        let corrected = self.n_corrected_readings(reading).into_iter().next()?;
        // No fall back to the repaired kana here, unlike the candidate
        // list: showing raw kana where a conversion stood is a downgrade,
        // not a suggestion.
        let text = self.model_candidates(&corrected, 1).into_iter().next()?;
        let typed = typed.to_string();
        (self.weigh(reading, Some(&typed), &corrected, &text) == Verdict::Replace).then_some(text)
    }

    /// Weigh one repair against the typing. `Offer` whenever there is no
    /// verdict to be had — the feature off, a reading too short to judge,
    /// no model, a scoring failure — so an unknown never rewrites text.
    fn weigh(
        &self,
        reading: &str,
        typed_text: Option<&String>,
        corrected: &str,
        corrected_text: &str,
    ) -> Verdict {
        if !self.config.auto_correct_n || reading.chars().count() < MIN_AUTO_CORRECTION_CHARS {
            return Verdict::Offer;
        }
        let (Some(typed_text), Some(converter)) = (typed_text, self.converters.kanji.as_ref())
        else {
            return Verdict::Offer;
        };
        let (Ok(typed_nll), Ok(corrected_nll)) = (
            converter.score(reading, typed_text),
            converter.score(corrected, corrected_text),
        ) else {
            return Verdict::Offer;
        };
        debug!(
            "repair: {reading}→{typed_text} ({typed_nll:.4}) vs {corrected}→{corrected_text} ({corrected_nll:.4})"
        );
        if typed_nll > corrected_nll * AUTO_CORRECTION_RATIO {
            Verdict::Replace
        } else if corrected_nll > typed_nll * REPAIR_NOISE_RATIO {
            Verdict::Drop
        } else {
            Verdict::Offer
        }
    }

    /// Classify the unresolved romaji tail for predictive lookup.
    fn tail_constraint(&self, pending: &str) -> TailConstraint {
        if pending.is_empty() {
            return TailConstraint::Unconstrained;
        }
        let expansions = self.converters.romaji.pending_expansions(pending);
        if expansions.is_empty() {
            TailConstraint::Dead
        } else {
            TailConstraint::Narrow(expansions)
        }
    }

    /// Build the mixed candidate list, deduped in priority order:
    /// Learning → User Dictionary → Model → System Dictionary → Fallback.
    ///
    /// `base`/`pending` split the reading for the dictionary lookup (the
    /// unresolved romaji tail narrows prediction).
    pub(super) fn build_conversion_candidates(
        &mut self,
        reading: &str,
        base: &str,
        pending: &str,
        num_candidates: usize,
        learning: LearningLookup,
    ) -> Vec<AnnotatedCandidate> {
        // No converter (still loading in the background, or loading failed)
        // just means no model candidates: symbol-only and early keystrokes
        // still get dictionary/rewriter/fallback candidates. Loading here
        // synchronously would block the key-event thread on the download.
        let candidates = self.model_candidates(reading, num_candidates);

        let hiragana = reading.to_string();
        let katakana = karukan_engine::hiragana_to_katakana(reading);

        // Priority: Learning → User Dictionary → Model → System Dictionary → Fallback
        let mut builder = CandidateBuilder::new();

        // 1. Learning cache candidates (highest priority).
        //    Force-inserted so they win against duplicate text from later sources.
        //    Skipped when the caller asks for a learning-free conversion (Tab key).
        if learning == LearningLookup::Use {
            for c in self.lookup_learning_candidates(reading) {
                // Exact matches have reading == input reading; use None to avoid redundancy
                let cand_reading = c.reading.filter(|r| r != reading);
                builder.push_force(
                    AnnotatedCandidate::new(c.text, CandidateSource::Learning)
                        .with_reading(cand_reading),
                );
            }
        }

        // 2. User dictionary candidates (system dictionary follows the model
        //    in step 4, so the two are split here).
        let (user_dict, system_dict): (Vec<_>, Vec<_>) = self
            .search_dictionaries(
                base,
                pending,
                usize::MAX,
                usize::MAX,
                MIN_PREDICTIVE_PREFIX_CHARS,
                None,
            )
            .into_iter()
            .partition(|ac| ac.source == CandidateSource::UserDictionary);
        for ac in user_dict {
            builder.push(ac);
        }

        // 3. Model inference results, headed by a repair the model is
        //    far surer of than the reading it was handed.
        let repairs = self.weighed_repairs(reading, &candidates);
        if candidates.is_empty() {
            // No literal fallback in emoji mode: `:smile` must not outrank
            // the 😄 surfaced by the rewriter step below.
            if builder.is_empty() && self.mode.current() != InputMode::Emoji {
                builder.push(AnnotatedCandidate::new(
                    hiragana.clone(),
                    CandidateSource::Fallback,
                ));
            }
        } else {
            // A replacement heads the model's own answer: it is what the
            // typing meant, so it is what Space should land on. Learning
            // and the user's dictionary still outrank it — those are
            // things the user said outright.
            for (text, _) in repairs.iter().filter(|(_, v)| *v == Verdict::Replace) {
                builder.push(
                    AnnotatedCandidate::new(text.clone(), CandidateSource::Model)
                        .with_description(Some(CORRECTION_LABEL.to_string())),
                );
            }
            for text in candidates {
                builder.push(AnnotatedCandidate::new(text, CandidateSource::Model));
            }
        }

        // 3b. The repairs that were not sure enough to replace anything
        //     ride below the model's answer, so a guess can never be what
        //     Space selects first. No `with_reading` override: a committed
        //     correction learns under the reading that was *typed*, which
        //     is what makes the same slip land right the next time — the
        //     repaired reading is one the user already types correctly
        //     when they type it at all.
        for (text, _) in repairs.iter().filter(|(_, v)| *v == Verdict::Offer) {
            builder.push(
                AnnotatedCandidate::new(text.clone(), CandidateSource::Model)
                    .with_description(Some(CORRECTION_LABEL.to_string())),
            );
        }

        // 4. System dictionary candidates
        for ac in system_dict {
            builder.push(ac);
        }

        // 5/6. Hiragana/katakana fallback + rewriter variants. Emoji mode
        // shows rewriter (emoji) candidates only — no kana pair, like an
        // emoji picker; Enter in Composing still commits the literal query.
        if self.mode.current() != InputMode::Emoji {
            builder.push(AnnotatedCandidate::new(hiragana, CandidateSource::Fallback));
            builder.push(AnnotatedCandidate::new(katakana, CandidateSource::Fallback));
        }
        // Date/time candidates sit above the width/kana variants; like the
        // rewriters they derive from the typed reading alone.
        for (variant, description) in self.date_variants(reading) {
            builder.push(
                AnnotatedCandidate::new(variant, CandidateSource::Date)
                    .with_description(description),
            );
        }
        // Rewriters run on the typed reading only; running them on other
        // sources' candidates would emit variants nobody asked for.
        for (variant, description) in self.rewriter_variants(reading) {
            builder.push(
                AnnotatedCandidate::new(variant, CandidateSource::Rewriter)
                    .with_description(description),
            );
        }

        // 7. Back-fill descriptions. Symbol names are Fallback-only —
        //    model/dict/learning candidates must not inherit labels like
        //    「金 = 部首」 — while width annotations (`[全]カタカナ`) apply to
        //    any pure-kana candidate that still has none.
        for c in &mut builder.candidates {
            if c.description.is_some() {
                continue;
            }
            let symbol = (c.source == CandidateSource::Fallback)
                .then(|| karukan_engine::symbol_description(&c.text))
                .flatten();
            c.description = symbol
                .or_else(|| width_annotation(&c.text))
                .map(str::to_string);
        }

        builder.into_candidates()
    }

    /// Look up learning cache candidates for a reading (exact + prefix match, max 3).
    ///
    /// Returns candidates from the learning cache suitable for auto-suggest display.
    pub(super) fn lookup_learning_candidates(&self, reading: &str) -> Vec<Candidate> {
        self.lookup_learning(reading, "", MAX_LEARNING_CANDIDATES)
    }

    /// Full learning history for `reading` (exact + prefix, uncapped),
    /// narrowed by the unresolved romaji tail like the dictionary lookup —
    /// an exact hit on the base must not swallow the typed tail.
    pub(super) fn lookup_learning_history(&self, reading: &str, pending: &str) -> Vec<Candidate> {
        self.lookup_learning(reading, pending, usize::MAX)
    }

    fn lookup_learning(&self, reading: &str, pending: &str, max: usize) -> Vec<Candidate> {
        let Some(cache) = &self.learning else {
            return vec![];
        };
        let constraint = self.tail_constraint(pending);
        if matches!(constraint, TailConstraint::Dead) {
            return vec![];
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut seen = HashSet::new();

        // Exact match — only when no romaji tail is pending (an exact hit
        // on the base would ignore the typed tail)
        if pending.is_empty() {
            for (surface, _score) in cache.lookup(reading) {
                if candidates.len() >= max {
                    break;
                }
                if seen.insert(surface.clone()) {
                    candidates.push(Candidate {
                        text: surface,
                        reading: Some(reading.to_string()),
                        source: Some(CandidateSource::Learning),
                        description: None,
                    });
                }
            }
        }

        // Prefix match (predictive), narrowed to the kana the tail can
        // still become — mirrors the dictionary's expanded search
        for (full_reading, surface, _score) in cache.prefix_lookup(reading) {
            if candidates.len() >= max {
                break;
            }
            if full_reading == reading {
                continue;
            }
            if let TailConstraint::Narrow(expansions) = &constraint {
                let rest = full_reading.strip_prefix(reading).unwrap_or(&full_reading);
                if !expansions.iter().any(|e| rest.starts_with(e.as_str())) {
                    continue;
                }
            }
            if seen.insert(surface.clone()) {
                candidates.push(Candidate {
                    text: surface,
                    reading: Some(full_reading),
                    source: Some(CandidateSource::Learning),
                    description: None,
                });
            }
        }

        candidates
    }

    /// Dictionary candidates for the composing suggestion list (one page).
    pub(super) fn lookup_dict_candidates(&self, reading: &str) -> Vec<Candidate> {
        let pending = self.input_buf.pending();
        self.search_dictionaries(
            reading,
            &pending,
            CandidateList::DEFAULT_PAGE_SIZE,
            MAX_PREDICTIVE_SUGGESTIONS,
            MIN_PREDICTIVE_PREFIX_CHARS,
            None,
        )
        .into_iter()
        .map(|ac| ac.into_candidate(reading))
        .collect()
    }

    /// Rewriter variants for `reading`, as `(text, description)` pairs.
    ///
    /// In emoji mode only the emoji rewriter runs: `:smile` is a query, and
    /// another rewriter's width variant (`：ｓｍｉｌｅ`) would head the
    /// picker and be what Enter commits.
    pub(super) fn rewriter_variants(&self, reading: &str) -> Vec<RewriteOutput> {
        if self.mode.current() == InputMode::Emoji {
            return EmojiRewriter.rewrite(reading);
        }
        self.converters
            .rewriters
            .rewrite_all(&[reading.to_string()])
    }

    /// Date/time candidates for `reading` (`[date]` phrases). None in emoji
    /// mode — the picker shows emojis only.
    pub(super) fn date_variants(&self, reading: &str) -> Vec<RewriteOutput> {
        if self.mode.current() == InputMode::Emoji {
            return Vec::new();
        }
        self.converters.date.rewrite(reading)
    }

    /// Build rule-based rewriter variants for the reading itself (e.g. for
    /// symbol input `「` → `『`, `【`, `（`, ...), date/time candidates first.
    /// Used in the auto-suggest path so users see mozc-style symbol variants
    /// without pressing Space first, and as the body of the Ctrl+R view.
    pub(super) fn lookup_rewriter_variants(&self, reading: &str) -> Vec<Candidate> {
        let as_candidate = |source: CandidateSource| {
            move |(text, description): RewriteOutput| Candidate {
                text,
                reading: Some(reading.to_string()),
                source: Some(source),
                description,
            }
        };
        self.date_variants(reading)
            .into_iter()
            .map(as_candidate(CandidateSource::Date))
            .chain(
                self.rewriter_variants(reading)
                    .into_iter()
                    .map(as_candidate(CandidateSource::Rewriter)),
            )
            .collect()
    }

    pub(super) fn process_key_conversion(&mut self, key: &KeyEvent) -> EngineResult {
        // Alt chords pass through before any binding matches: Alt+Tab must
        // navigate and Alt+Return must not commit.
        if key.modifiers.alt_key {
            return EngineResult::not_consumed();
        }
        match key.keysym {
            Keysym::RETURN => self.commit_conversion(),
            Keysym::ESCAPE => self.cancel_conversion(),
            // Shift+Tab (ISO_Left_Tab on X11) and Shift+Space step back the
            // way Tab and Space step forward: mozc-compatible muscle memory.
            Keysym::ISO_LEFT_TAB => self.prev_candidate(),
            Keysym::TAB | Keysym::SPACE if key.modifiers.shift_key => self.prev_candidate(),
            Keysym::SPACE | Keysym::DOWN | Keysym::TAB => self.next_candidate(),
            Keysym::UP => self.prev_candidate(),
            Keysym::PAGE_DOWN => self.next_candidate_page(),
            Keysym::PAGE_UP => self.prev_candidate_page(),
            // Ctrl+Backspace / Ctrl+Delete: delete the selected learning
            // candidate (the Mac "delete" key is Backspace). A non-learning
            // selection consumes the chord as a no-op.
            Keysym::DELETE | Keysym::BACKSPACE if key.modifiers.control_key => {
                if self.selected_is_deletable() {
                    self.delete_selected_candidate_from_history()
                } else {
                    EngineResult::consumed()
                }
            }
            // Inside a narrowed view Backspace shrinks the reading and
            // stays in the view — the mirror of typing-refine, so the list
            // re-expands as the query shrinks. Without a filter it returns
            // to the composition as before.
            Keysym::BACKSPACE if self.state.filter().is_some() => {
                self.refine_through_composing(key)
            }
            // Backspace cancels back to the composition, like Escape.
            Keysym::BACKSPACE => self.cancel_conversion(),
            // Caret keys drop back to editing, the same way a caret move
            // ends the live-conversion display while composing: the
            // conversion (and its source filter) dissolves and the raw
            // reading gets the caret. Delegated to the composing handler so
            // the two states cannot drift apart.
            Keysym::LEFT | Keysym::RIGHT | Keysym::HOME | Keysym::END => {
                self.in_composing(false, |e| e.process_key_composing(key))
            }
            _ => {
                // Ctrl+N / Ctrl+P: emacs-style candidate navigation
                if key.modifiers.control_key {
                    match key.keysym {
                        Keysym::KEY_N | Keysym::KEY_N_UPPER => return self.next_candidate(),
                        Keysym::KEY_P | Keysym::KEY_P_UPPER => return self.prev_candidate(),
                        // Ctrl+R / Ctrl+T: cycle the source filter. Both
                        // keysym cases — some environments fold Shift into
                        // an uppercase keysym; direction must not change.
                        Keysym::KEY_R | Keysym::KEY_R_UPPER => {
                            return self.cycle_candidate_filter(FilterDirection::Backward);
                        }
                        Keysym::KEY_T | Keysym::KEY_T_UPPER => {
                            return self.cycle_candidate_filter(FilterDirection::Forward);
                        }
                        // Ctrl+J: split at the caret and rebuild, so the
                        // alternatives cover only the text after the break.
                        Keysym::KEY_J | Keysym::KEY_J_UPPER => {
                            return self.rebreak_conversion();
                        }
                        // Ctrl+L: drop the conversion and hand back the
                        // typing as Latin text — the composition behind
                        // the candidates still holds the keystrokes.
                        Keysym::KEY_L | Keysym::KEY_L_UPPER => {
                            return self.convert_to_alphabet();
                        }
                        // Ctrl+A/B/E/F: the same caret moves as while
                        // composing, dropping back to editing like the
                        // arrow keys above.
                        Keysym::KEY_A
                        | Keysym::KEY_A_UPPER
                        | Keysym::KEY_B
                        | Keysym::KEY_B_UPPER
                        | Keysym::KEY_E
                        | Keysym::KEY_E_UPPER
                        | Keysym::KEY_F
                        | Keysym::KEY_F_UPPER => {
                            return self.in_composing(false, |e| e.process_key_composing(key));
                        }
                        _ => {}
                    }

                    // Ctrl+Y/U/I/O: jump straight to one source's view.
                    if let Some(source) = source_for_key(key.keysym) {
                        return self.jump_to_source(source);
                    }

                    // Ctrl+1..9: select and commit that candidate. Bare
                    // digits refine below like any printable character, so
                    // typing numbers never conflicts with selection.
                    if let Some(digit) = key.keysym.digit_value() {
                        return self.select_shown_candidate(digit);
                    }
                }

                // A printable character refines instead of committing:
                // the reading grows and the suggestion rewrites in place,
                // keeping any active source filter.
                if key.to_char().is_some() && !key.modifiers.control_key {
                    return self.refine_through_composing(key);
                }

                // Everything else is consumed as a no-op — leaked chords
                // would let the app act on them mid-conversion (e.g. a
                // browser reloading on Ctrl+R).
                EngineResult::consumed()
            }
        }
    }

    /// Feed a refining keystroke (printable char, Backspace) through the
    /// composing path, then re-enter the conversion with the previous
    /// source filter if one was active. With a filter the composing render
    /// is discarded, so its auto-suggest inference is suppressed.
    fn refine_through_composing(&mut self, key: &KeyEvent) -> EngineResult {
        let filter = self.state.filter();
        let result =
            self.in_composing(filter.is_some(), |engine| engine.process_key_composing(key));
        if let Some(source) = filter
            && matches!(self.state, InputState::Composing { .. })
        {
            return self.start_conversion_with_filter(source);
        }
        result
    }

    /// Insert a chunk break at the caret without leaving the conversion.
    /// The span is the last chunk, so breaking narrows what the beam
    /// covers; the rebuilt list keeps the active source filter. The
    /// intermediate composing render is discarded, so its auto-suggest
    /// inference is suppressed.
    fn rebreak_conversion(&mut self) -> EngineResult {
        let filter = self.state.filter();
        self.in_composing(true, |engine| engine.insert_chunk_break());
        match filter {
            Some(source) => self.start_conversion_with_filter(source),
            None => self.start_conversion(LearningLookup::Use),
        }
    }

    /// Drop back to the untouched composition and run `edit` there. Set
    /// `discard_render` when the caller rebuilds the conversion afterwards:
    /// the composing render is thrown away, so its auto-suggest inference
    /// would be pure waste. The flag lives and dies inside this call, so no
    /// other path can inherit it.
    fn in_composing<R>(&mut self, discard_render: bool, edit: impl FnOnce(&mut Self) -> R) -> R {
        self.set_composing_state();
        self.suppress_suggest = discard_render;
        let out = edit(self);
        self.suppress_suggest = false;
        out
    }

    /// Get selected text and reading from conversion state, or None if not in conversion
    pub(super) fn selected_conversion_info(&self) -> Option<(String, Option<String>)> {
        match &self.state {
            InputState::Conversion {
                candidates,
                reading,
                ..
            } => {
                // An empty (source-filtered) view displays the raw reading
                // as its preedit, so that is what committing produces —
                // never an empty commit that would eat the composition.
                let text = candidates.selected_text().unwrap_or(reading).to_string();
                // The reading rides along solely for the learning record;
                // a non-learnable source (a date is stale tomorrow) yields
                // None so nothing is recorded.
                let reading = candidates
                    .selected()
                    .filter(|c| c.source.is_none_or(|s| s.is_learnable()))
                    .and_then(|c| c.reading.clone());
                Some((text, reading))
            }
            _ => None,
        }
    }

    /// Record a selection in the learning cache. No-op in emoji mode — the
    /// buffer is a `:query`, not a kana reading, and would corrupt the
    /// kana-keyed cache.
    pub(super) fn record_learning(&mut self, reading: &str, surface: &str) {
        if self.mode.current() == InputMode::Emoji {
            return;
        }
        if let Some(cache) = &mut self.learning {
            cache.record(reading, surface);
        }
    }

    /// Record the committed conversion in the learning cache and end the
    /// composition.
    pub(super) fn finish_conversion(&mut self, text: &str, reading: &Option<String>) {
        if let Some(reading) = reading {
            self.record_learning(reading, text);
        }
        self.end_composition();
    }

    /// Commit the current conversion
    fn commit_conversion(&mut self) -> EngineResult {
        let Some((text, reading)) = self.selected_conversion_info() else {
            return EngineResult::not_consumed();
        };

        if text.is_empty() {
            return EngineResult::consumed();
        }

        self.finish_conversion(&text, &reading);

        EngineResult::consumed()
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::HideAuxText)
            .with_action(EngineAction::Commit(text))
    }

    /// Whether the selected candidate can be removed from the learning
    /// history. False when nothing is selected, so the delete chord stays
    /// inert outside the case it is meant for.
    fn selected_is_deletable(&self) -> bool {
        self.state
            .candidates()
            .and_then(|c| c.selected())
            .is_some_and(Candidate::is_deletable)
    }

    /// Delete the selected learning candidate and its prefix twins from the
    /// history, then rebuild the conversion in place — dedup hid any other
    /// source's copy of the surface, and only a rebuild brings it back.
    /// The caller guards deletability ([`Self::selected_is_deletable`]).
    fn delete_selected_candidate_from_history(&mut self) -> EngineResult {
        let Some(surface) = self
            .state
            .candidates()
            .and_then(|c| c.selected())
            .map(|c| c.text.clone())
        else {
            return EngineResult::consumed();
        };
        // Remove by the typed reading: every entry surfacing this row has
        // it as a prefix, so the row and its twins clear together.
        let Some(reading) = self.state.reading().map(str::to_string) else {
            return EngineResult::consumed();
        };
        let removed = self
            .learning
            .as_mut()
            .is_some_and(|cache| cache.remove_suggestion(&reading, &surface));
        if !removed {
            return EngineResult::consumed();
        }
        debug!("deleted learning entry: {} -> {}", reading, surface);

        // Keep the filter and cursor so consecutive deletes stay in the
        // narrowed view and chew through the list top-down.
        let prev_filter = self.state.filter();
        let prev_cursor = self.state.candidates().map(|c| c.cursor()).unwrap_or(0);

        let candidates = self.build_conversion_candidates(
            &reading,
            &reading,
            "",
            self.config.num_candidates,
            LearningLookup::Use,
        );
        if candidates.is_empty() {
            return self.cancel_conversion();
        }
        let candidate_list = self.to_conversion_candidate_list(candidates, &reading);
        let mut result = self.enter_conversion_state(&reading, candidate_list);

        if let Some(source) = prev_filter {
            result = self.apply_candidate_filter(source);
        }
        if self.state.candidates().is_some_and(|c| !c.is_empty()) {
            return self.navigate_candidate(|c| {
                c.set_cursor(prev_cursor);
                true
            });
        }
        result
    }
    pub(super) fn cancel_conversion(&mut self) -> EngineResult {
        if !matches!(self.state, InputState::Conversion { .. }) {
            return EngineResult::not_consumed();
        }

        if self.input_buf.is_empty() {
            self.state = InputState::Empty;
            return EngineResult::consumed()
                .with_action(EngineAction::UpdatePreedit(Preedit::new()))
                .with_action(EngineAction::HideCandidates)
                .with_action(EngineAction::HideAuxText);
        }

        // The composition was left untouched when the conversion started:
        // just come back to it, pending romaji still live
        let preedit = self.set_composing_state();

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::UpdateAuxText(self.format_aux_composing()))
    }

    /// Navigate candidates with the given operation, then update preedit
    fn navigate_candidate(&mut self, op: impl FnOnce(&mut CandidateList) -> bool) -> EngineResult {
        let (selected_text, candidates) = {
            let Some(candidates) = self.state.candidates_mut() else {
                return EngineResult::not_consumed();
            };
            // Nothing to navigate in an empty (source-filtered) view; keep
            // the reading preedit instead of blanking it.
            if candidates.is_empty() {
                return EngineResult::consumed();
            }
            op(candidates);
            let text = candidates.selected_text().unwrap_or("").to_string();
            (text, candidates.clone())
        };
        self.update_conversion_preedit(&selected_text, candidates)
    }

    /// Select next candidate
    fn next_candidate(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::move_next)
    }

    /// Select previous candidate
    fn prev_candidate(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::move_prev)
    }

    /// Go to next candidate page
    fn next_candidate_page(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::next_page)
    }

    /// Go to previous candidate page
    fn prev_candidate_page(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::prev_page)
    }

    /// Select and commit the candidate at `page_index` (0-based) within the
    /// current page, like pressing the digit key `page_index + 1`. Not
    /// consumed unless a candidate list is active (Conversion state).
    pub fn select_candidate_on_page(&mut self, page_index: usize) -> EngineResult {
        let start = std::time::Instant::now();
        self.metrics.conversion_ms = 0;
        let result = self.select_shown_candidate(page_index + 1);
        self.metrics.process_key_ms = start.elapsed().as_millis() as u64;
        result
    }

    /// Update preedit after candidate selection change
    fn update_conversion_preedit(
        &mut self,
        selected_text: &str,
        candidates: CandidateList,
    ) -> EngineResult {
        let preedit = Preedit::with_text_highlighted(selected_text);

        if let Some(p) = self.state.preedit_mut() {
            *p = preedit.clone();
        }

        let reading = candidates
            .selected()
            .and_then(|c| c.reading.clone())
            .unwrap_or_default();
        let aux = self.format_aux_conversion_with_page(&reading, Some(&candidates));

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::ShowCandidates(candidates))
            .with_action(EngineAction::UpdateAuxText(aux))
    }
}
