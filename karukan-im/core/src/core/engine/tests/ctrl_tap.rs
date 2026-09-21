//! A lone Ctrl tap comes back to kana, the way 変換 and the right-hand
//! modifiers do — the way out of the Alphabet mode that Shift+letter and
//! Ctrl+L put you in.

use super::*;

/// Press and release Ctrl with nothing in between.
fn tap_ctrl(engine: &mut InputMethodEngine) -> EngineResult {
    engine.process_key(&press_key(Keysym::CONTROL_L));
    engine.process_key(&release_key(Keysym::CONTROL_L))
}

#[test]
fn a_ctrl_tap_leaves_the_alphabet_mode_shift_opened() {
    let mut engine = InputMethodEngine::new();
    engine.process_key(&press_shift('A'));
    assert_eq!(engine.mode.current(), InputMode::Alphabet);

    tap_ctrl(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Hiragana);

    // Kana again, and the `A` already typed is left alone.
    type_keys(&mut engine, "ka");
    assert_eq!(engine.preedit().unwrap().text(), "Aか");
}

#[test]
fn a_ctrl_tap_leaves_the_alphabet_mode_ctrl_l_opened() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");
    engine.process_key(&press_ctrl(Keysym::KEY_L));
    assert_eq!(engine.preedit().unwrap().text(), "hello");
    assert_eq!(engine.mode.current(), InputMode::Alphabet);

    tap_ctrl(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Hiragana);

    type_keys(&mut engine, "ka");
    assert_eq!(engine.preedit().unwrap().text(), "helloか");
}

#[test]
fn the_tap_release_is_passed_through() {
    // Consuming it would leave the application thinking Ctrl is still
    // down — but the mode change still has to reach the UI.
    let mut engine = InputMethodEngine::new();
    engine.process_key(&press_shift('A'));

    engine.process_key(&press_key(Keysym::CONTROL_L));
    let result = engine.process_key(&release_key(Keysym::CONTROL_L));
    assert!(!result.consumed);
    assert!(last_aux_text(&result).is_some(), "{:?}", result.actions);
}

#[test]
fn a_ctrl_tap_leaves_katakana_mode_too() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "ka");
    engine.process_key(&press_ctrl(Keysym::KEY_K));
    assert_eq!(engine.mode.current(), InputMode::Katakana);

    tap_ctrl(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
    // The katakana already on screen was baked, not reverted.
    assert_eq!(engine.preedit().unwrap().text(), "カ");
}

#[test]
fn a_ctrl_chord_is_not_a_tap() {
    // Ctrl+L must still convert: the press that opens the chord cannot be
    // allowed to switch modes, and its release is not a tap either.
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "hello");

    engine.process_key(&press_key(Keysym::CONTROL_L));
    engine.process_key(&press_ctrl(Keysym::KEY_L));
    engine.process_key(&release_key(Keysym::KEY_L));
    engine.process_key(&release_key(Keysym::CONTROL_L));

    assert_eq!(engine.preedit().unwrap().text(), "hello");
    assert_eq!(engine.mode.current(), InputMode::Alphabet);
}

#[test]
fn ctrl_with_another_modifier_is_not_a_tap() {
    let mut engine = InputMethodEngine::new();
    engine.process_key(&press_shift('A'));

    // Ctrl pressed while Shift is down: a chord in the making.
    engine.process_key(&press_shift_key(Keysym::CONTROL_L));
    engine.process_key(&release_key(Keysym::CONTROL_L));
    assert_eq!(engine.mode.current(), InputMode::Alphabet);
}

#[test]
fn a_ctrl_tap_in_kana_changes_nothing() {
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "ka");

    let result = tap_ctrl(&mut engine);
    assert!(!result.consumed);
    assert!(result.actions.is_empty(), "{:?}", result.actions);
    assert_eq!(engine.preedit().unwrap().text(), "か");
}

#[test]
fn a_ctrl_tap_mid_conversion_leaves_the_conversion_alone() {
    // The kana modes cannot toggle under an open candidate window: the
    // reading would be katakana-baked behind the user's back.
    let mut engine = InputMethodEngine::new();
    type_keys(&mut engine, "ka");
    engine.process_key(&press_ctrl(Keysym::KEY_K));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    tap_ctrl(&mut engine);
    assert_eq!(engine.mode.current(), InputMode::Katakana);
}
