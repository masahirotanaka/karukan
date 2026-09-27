//! A latin word typed inline, and the alphabet spellings that no longer
//! answer a kana reading.

use super::*;

/// Engine with a system dictionary whose latin surfaces are the word list
/// the split needs, and no model — so what comes out is the split itself,
/// not an inference on top of it.
fn latin_engine() -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.live.enabled = true;
    engine.dicts.system = Some(dict_from_json(
        r#"[
            {"reading":"くろーず","candidates":[{"surface":"クローズ","score":1.0},{"surface":"close","score":2.0}]},
            {"reading":"えぴっく","candidates":[{"surface":"エピック","score":1.0},{"surface":"EPIC","score":2.0}]},
            {"reading":"らすと","candidates":[{"surface":"rust","score":1.0}]},
            {"reading":"えぴっくはい","candidates":[{"surface":"Epik High","score":1.0}]},
            {"reading":"くろ","candidates":[{"surface":"cl","score":1.0}]}
        ]"#,
    ));
    engine.refresh_latin_words();
    engine
}

fn preedit(result: &EngineResult) -> String {
    result
        .actions
        .iter()
        .rev()
        .find_map(|a| match a {
            EngineAction::UpdatePreedit(p) => Some(p.text().to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn candidates(result: &EngineResult) -> Vec<String> {
    result
        .actions
        .iter()
        .rev()
        .find_map(|a| match a {
            EngineAction::ShowCandidates(list) => {
                Some(list.candidates().iter().map(|c| c.text.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn a_latin_word_typed_inline_reads_as_itself() {
    // `closesite` romanizes to 「cぉせして」 — the `c` reached no rule —
    // and `close` is a surface the dictionary answers 「くろーず」 with.
    let mut engine = latin_engine();
    let result = type_keys(&mut engine, "closesite");
    assert_eq!(preedit(&result), "closeして");
}

#[test]
fn the_split_survives_the_space_that_opens_a_conversion() {
    let mut engine = latin_engine();
    type_keys(&mut engine, "closesite");
    let result = engine.process_key(&press_key(Keysym::SPACE));
    assert_eq!(preedit(&result), "closeして");
    assert_eq!(
        candidates(&result).first().map(String::as_str),
        Some("closeして")
    );
}

#[test]
fn ordinary_romaji_is_never_reinterpreted() {
    // 「くろーず」 romanizes cleanly, so there is nothing to explain and
    // the dictionary's own `close` stays a candidate, not the reading.
    let mut engine = latin_engine();
    let result = type_keys(&mut engine, "kuro-zusite");
    assert_eq!(preedit(&result), "くろーずして");
}

#[test]
fn an_uppercase_word_takes_the_kana_typed_onto_it() {
    // Shift+E opens alphabet mode; the case change is the boundary.
    let mut engine = latin_engine();
    engine.process_key(&press_shift('E'));
    let result = type_keys(&mut engine, "PICha");
    assert_eq!(preedit(&result), "EPICは");
}

#[test]
fn an_alphabet_spelling_no_longer_answers_a_kana_reading() {
    // 「くろーず」 → `close` is in the dictionary and stays out of the
    // candidates: alphabet is what alphabet keystrokes produce.
    let mut engine = latin_engine();
    let result = type_keys(&mut engine, "kuro-zu");
    let shown = candidates(&result);
    assert!(!shown.iter().any(|c| c.contains("close")), "got {shown:?}");
    assert!(shown.iter().any(|c| c == "クローズ"), "got {shown:?}");
}

#[test]
fn a_learned_alphabet_spelling_does_not_come_back() {
    // Picking `EPIC` once for 「えぴっく」 must not make it what every
    // later 「えぴっく」 converts to.
    let mut engine = engine_with_learned("えぴっく", "EPIC");
    let shown = candidates(&type_keys(&mut engine, "epikku"));
    assert!(!shown.iter().any(|c| c == "EPIC"), "got {shown:?}");
}

#[test]
fn the_setting_puts_the_alphabet_spelling_back() {
    let mut engine = latin_engine();
    engine.config.alphabet_from_kana = true;
    let shown = candidates(&type_keys(&mut engine, "kuro-zu"));
    assert!(shown.iter().any(|c| c == "close"), "got {shown:?}");
}

#[test]
fn only_latin_the_reading_never_had_is_dropped() {
    let engine = latin_engine();
    // The alphabet spelling of a kana reading.
    assert!(engine.drops_alphabet_surface("くろーずして", "closeして"));
    // Latin the reading was typed with — a composition that switched
    // modes mid-word — spells back what is already there.
    assert!(!engine.drops_alphabet_surface("firebaseぷろじぇくと", "firebaseプロジェクト"));
    // A reading with no kana in it is alphabet typing outright, which is
    // what keeps a raw-keystroke dictionary shortcut reachable.
    assert!(!engine.drops_alphabet_surface("m", "masahiro@example.com"));
    // One stray romaji keystroke does not license a whole word.
    assert!(engine.drops_alphabet_surface("えぴっくyは", "EPICyは"));
}

#[test]
fn an_emoji_query_is_not_a_word_typed_inline() {
    // 「:smile」 is latin behind a passed-through `:` — the very shape the
    // split looks for — but it is a query the picker answers.
    let mut engine = latin_engine();
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"すまいる","candidates":[{"surface":"smile","score":1.0}]}]"#,
    ));
    engine.refresh_latin_words();
    let result = type_keys(&mut engine, ":smile");
    assert_eq!(preedit(&result), ":smile");
}
