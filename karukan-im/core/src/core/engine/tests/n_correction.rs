//! Typing corrections for the keystroke people drop: the second `n` of
//! ん. The reading that was typed keeps its place at the head of the
//! list; the repaired one rides below it, annotated 「もしかして」.

use super::*;

/// Candidate texts with the 「もしかして」 annotation, in list order.
fn corrections(engine: &InputMethodEngine) -> Vec<String> {
    let InputState::Conversion { candidates, .. } = engine.state() else {
        return Vec::new();
    };
    candidates
        .candidates()
        .iter()
        .filter(|c| c.description.as_deref() == Some("もしかして"))
        .map(|c| c.text.clone())
        .collect()
}

/// Every candidate text, in list order.
fn all(engine: &InputMethodEngine) -> Vec<String> {
    let InputState::Conversion { candidates, .. } = engine.state() else {
        return Vec::new();
    };
    candidates
        .candidates()
        .iter()
        .map(|c| c.text.clone())
        .collect()
}

/// Type `keys`, then Space. No model is loaded, so a correction shows up
/// as the repaired kana — which is what it converts to here.
fn convert(keys: &str) -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_keys(&mut engine, keys);
    engine.process_key(&press_key(Keysym::SPACE));
    engine
}

#[test]
fn a_dropped_n_before_a_vowel_is_offered_back() {
    // `kanni` → かんい; typed one `n` short it reads かに.
    let engine = convert("kani");
    assert_eq!(corrections(&engine), ["かんい"]);
}

#[test]
fn a_dropped_n_before_na_row_is_offered_back() {
    // こんにちは needs three `n`s; two of them reads こんいちは.
    let engine = convert("konnichiha");
    assert!(
        corrections(&engine).contains(&"こんにちは".to_string()),
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn a_dropped_n_before_ya_row_is_offered_back() {
    // しんや typed as `shinya` reads しにゃ.
    let engine = convert("shinya");
    assert!(
        corrections(&engine).contains(&"しんや".to_string()),
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn the_typed_reading_still_heads_the_list() {
    // A correction is a guess: Space must never land on one first.
    let engine = convert("kani");
    let all = all(&engine);
    assert_eq!(all.first().map(String::as_str), Some("かに"), "{all:?}");
    let first_correction = all.iter().position(|t| t == "かんい").unwrap();
    assert!(first_correction > 0, "{all:?}");
}

#[test]
fn a_word_with_no_n_costs_nothing() {
    let engine = convert("sakura");
    assert!(
        corrections(&engine).is_empty(),
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn a_reading_already_right_is_not_second_guessed() {
    // `kannni` is かんに already; doubling an `n` there converts to
    // something else, but the typed reading itself is never repeated.
    let engine = convert("kanni");
    assert!(
        !corrections(&engine).contains(&"かんい".to_string()),
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn at_most_two_repairs_are_tried() {
    // Every `n` is a place the keystroke could have gone; the cap is what
    // keeps one Space from costing a model call per `n`.
    let engine = convert("nanonanona");
    assert!(
        corrections(&engine).len() <= 2,
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn committing_a_correction_learns_under_what_was_typed() {
    // The point of learning it: the same slip lands right next time.
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.learning = Some(LearningCache::new(LearningConfig::default()));
    type_keys(&mut engine, "kani");
    engine.process_key(&press_key(Keysym::SPACE));

    // Walk to the correction and commit it.
    let steps = all(&engine).iter().position(|t| t == "かんい").unwrap();
    for _ in 0..steps {
        engine.process_key(&press_key(Keysym::SPACE));
    }
    engine.process_key(&press_key(Keysym::RETURN));

    // Typing the same slip again offers it from the learning cache.
    let learned = engine.learning.as_ref().unwrap().lookup("かに");
    assert!(
        learned.iter().any(|(surface, _)| surface == "かんい"),
        "learned: {learned:?}"
    );
}

#[test]
fn a_repair_that_opens_with_n_is_never_offered() {
    // `nekogasukidesu` doubles its leading `n` into んえこがすきです —
    // no Japanese word opens with ん, and this is the one repair the
    // model scored *better* than 猫が好きです.
    let engine = convert("nekogasukidesu");
    assert!(
        !corrections(&engine).iter().any(|t| t.starts_with('ん')),
        "{:?}",
        corrections(&engine)
    );
}

#[test]
fn nothing_is_replaced_without_a_model() {
    // The head of the list is the model's answer to what was typed, and
    // with no model that is the reading itself. A repair can only ever
    // displace it on a score, never on its own.
    let engine = convert("kani");
    assert_eq!(all(&engine).first().map(String::as_str), Some("かに"));
}

#[test]
fn the_toggle_turns_replacement_off_but_keeps_the_candidate() {
    let mut engine = InputMethodEngine::with_config(EngineConfig {
        auto_correct_n: false,
        ..EngineConfig::default()
    });
    engine.converters.kanji = None;
    type_keys(&mut engine, "konnichiha");
    engine.process_key(&press_key(Keysym::SPACE));

    assert_eq!(all(&engine).first().map(String::as_str), Some("こんいちは"));
    assert!(
        corrections(&engine).contains(&"こんにちは".to_string()),
        "{:?}",
        corrections(&engine)
    );
}
