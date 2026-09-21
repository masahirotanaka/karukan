//! Ctrl+L: hand back what was typed, as Latin text (mozc's F10 / F9).

use super::*;

/// The preedit text an engine is currently showing.
fn preedit(engine: &InputMethodEngine) -> String {
    engine
        .preedit()
        .map(|p| p.text().to_string())
        .unwrap_or_default()
}

fn ctrl_l(engine: &mut InputMethodEngine) -> EngineResult {
    engine.process_key(&press_ctrl(Keysym::KEY_L))
}

#[test]
fn ctrl_l_gives_back_the_typing() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    // Typed as kana, so the preedit is not the spelling.
    assert_ne!(preedit(&engine), "hello");

    let result = ctrl_l(&mut engine);
    assert!(result.consumed);
    assert_eq!(preedit(&engine), "hello");
    // Kana carries straight on — no mode key to get back to Japanese.
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
}

#[test]
fn kana_carries_on_after_ctrl_l() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    ctrl_l(&mut engine);

    type_keys(&mut engine, "ka");
    assert_eq!(preedit(&engine), "helloか");
}

#[test]
fn ctrl_l_ends_the_mode_shift_opened() {
    // Shift+letter's Alphabet is temporary, and the conversion ends it.
    let mut engine = InputMethodEngine::new();
    engine.process_key(&press_shift('A'));
    type_keys(&mut engine, "bc");
    assert_eq!(engine.mode.current(), InputMode::Alphabet);

    ctrl_l(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
}

#[test]
fn ctrl_l_leaves_a_deliberate_katakana_mode_alone() {
    // Katakana was picked outright (Ctrl+K), so it is not something the
    // conversion gets to undo — only a temporary mode is.
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "ka");
    engine.process_key(&press_ctrl(Keysym::KEY_K));

    ctrl_l(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Katakana);
}

#[test]
fn ctrl_l_reproduces_multi_key_rules() {
    // Rules that eat several keystrokes for one or two kana still know
    // what they were typed as: きょ came from `kyo`, ん from `nn`, っ
    // from the doubled consonant.
    for keys in ["kyou", "konnnichiha", "gakkou", "jishin", "nihongo"] {
        let mut engine = InputMethodEngine::new();
        type_keys(&mut engine, keys);
        ctrl_l(&mut engine);
        assert_eq!(preedit(&engine), keys, "round trip of {keys}");
    }
}

#[test]
fn ctrl_l_keeps_passed_through_keystrokes() {
    // `1` has no rule and settles as itself; `y` stays a live keystroke.
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "ka1y");
    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "ka1y");
}

#[test]
fn ctrl_l_walks_case_and_width() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");

    for expected in [
        "hello",
        "HELLO",
        "Hello",
        "ｈｅｌｌｏ",
        "ＨＥＬＬＯ",
        // …and round again, always re-cut from the original typing.
        "hello",
    ] {
        ctrl_l(&mut engine);
        assert_eq!(preedit(&engine), expected);
    }
}

#[test]
fn ctrl_l_names_the_form_in_the_aux_line() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");

    let aux = last_aux_text(&ctrl_l(&mut engine)).unwrap();
    assert!(aux.contains("[半]英字"), "aux was {aux}");
    let aux = last_aux_text(&ctrl_l(&mut engine)).unwrap();
    assert!(aux.contains("[半]英大文字"), "aux was {aux}");
}

#[test]
fn ctrl_l_on_digits_walks_width_only() {
    // Case does nothing to digits, so those forms drop out and the walk
    // is half ↔ full. The digits already read half-width, so the first
    // press goes straight to the form that differs.
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "123");
    assert_eq!(preedit(&engine), "123");

    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "１２３");
    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "123");
}

#[test]
fn typing_after_ctrl_l_restarts_the_walk() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    ctrl_l(&mut engine);
    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "HELLO");

    // Kana mode now, so this is a live keystroke — still shown as `x`,
    // since `x` alone has fired no rule yet.
    type_keys(&mut engine, "x");
    assert_eq!(preedit(&engine), "HELLOx");

    // The edit ended the walk: the next press re-cuts from what is there
    // now, skipping the form that would leave the text untouched.
    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "HELLOX");
}

#[test]
fn ctrl_l_works_from_the_candidate_window() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "kyou");
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    ctrl_l(&mut engine);
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(preedit(&engine), "kyou");
}

#[test]
fn commit_after_ctrl_l_returns_to_kana() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    ctrl_l(&mut engine);

    let result = engine.process_key(&press_key(Keysym::RETURN));
    let committed = result.actions.iter().find_map(|a| match a {
        EngineAction::Commit(text) => Some(text.clone()),
        _ => None,
    });
    assert_eq!(committed.as_deref(), Some("hello"));
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
}

#[test]
fn ctrl_l_survives_katakana_mode() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "konnpyu-ta");
    engine.process_key(&press_ctrl(Keysym::KEY_K));
    assert_eq!(engine.mode.current(), InputMode::Katakana);

    ctrl_l(&mut engine);
    assert_eq!(preedit(&engine), "konnpyu-ta");
}

#[test]
fn ctrl_l_on_an_empty_composition_is_passed_through() {
    let mut engine = InputMethodEngine::new();
    assert!(!ctrl_l(&mut engine).consumed);
}

#[test]
fn ctrl_shift_l_still_toggles_live_conversion() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    let before = engine.live.enabled;

    engine.process_key(&press_ctrl_shift(Keysym::KEY_L));
    assert_eq!(engine.live.enabled, !before);
    // …and left the composition alone.
    assert_ne!(preedit(&engine), "hello");
}
