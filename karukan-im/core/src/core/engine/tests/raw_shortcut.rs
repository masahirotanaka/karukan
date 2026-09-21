//! Latin shortcuts: a user dictionary entry read as the keystrokes
//! themselves (`m` → an address), answered while the composition is still
//! nothing but an unresolved romaji tail.

use super::*;

const SHORTCUTS: &str = r#"[
    {"reading":"m","candidates":[
        {"surface":"masahiro@asial.co.jp","score":0.0},
        {"surface":"masahiro.tanaka@gmail.com","score":0.0}
    ]},
    {"reading":"gm","candidates":[{"surface":"masahiro.tanaka@gmail.com","score":0.0}]},
    {"reading":"まさひろ","candidates":[{"surface":"正裕","score":0.0}]}
]"#;

fn shortcut_engine() -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.dicts.user = Some(dict_from_json(SHORTCUTS));
    engine
}

/// Texts of the candidate list the result put on screen.
fn shown(result: &EngineResult) -> Vec<String> {
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
fn a_lone_consonant_answers_from_the_user_dictionary() {
    // The first keystroke out of Empty only opens the composition — no
    // path there runs auto-suggest — so Space is what asks the question.
    let mut engine = shortcut_engine();
    engine.process_key(&press('m'));
    let result = engine.process_key(&press_key(Keysym::SPACE));

    let shown = shown(&result);
    assert_eq!(
        shown.iter().take(2).collect::<Vec<_>>(),
        ["masahiro@asial.co.jp", "masahiro.tanaka@gmail.com"],
        "shown: {shown:?}"
    );
}

#[test]
fn a_two_letter_shortcut_shows_without_space() {
    // From the second keystroke on, every edit refreshes the suggestion
    // window, so a shortcut longer than one letter answers as it is typed.
    let mut engine = shortcut_engine();
    let result = type_keys(&mut engine, "gm");

    let shown = shown(&result);
    assert_eq!(engine.preedit().unwrap().text(), "gm");
    assert_eq!(
        shown.first().map(String::as_str),
        Some("masahiro.tanaka@gmail.com"),
        "shown: {shown:?}"
    );
}

#[test]
fn the_shortcut_commits_what_was_selected() {
    let mut engine = shortcut_engine();
    engine.process_key(&press('m'));
    engine.process_key(&press_key(Keysym::SPACE));
    // Enter takes the selected candidate, which is the head of the list.
    let result = engine.process_key(&press_key(Keysym::RETURN));

    let committed = result.actions.iter().find_map(|a| match a {
        EngineAction::Commit(text) => Some(text.clone()),
        _ => None,
    });
    assert_eq!(committed.as_deref(), Some("masahiro@asial.co.jp"));
}

#[test]
fn the_second_address_is_one_step_away() {
    let mut engine = shortcut_engine();
    engine.process_key(&press('m'));
    engine.process_key(&press_key(Keysym::SPACE));
    engine.process_key(&press_key(Keysym::SPACE));
    let result = engine.process_key(&press_key(Keysym::RETURN));

    let committed = result.actions.iter().find_map(|a| match a {
        EngineAction::Commit(text) => Some(text.clone()),
        _ => None,
    });
    assert_eq!(committed.as_deref(), Some("masahiro.tanaka@gmail.com"));
}

#[test]
fn the_next_keystroke_takes_the_shortcut_away() {
    // `m` is a shortcut only while it is still a bare keystroke: `ma`
    // settles to ま and the composition is an ordinary reading again.
    let mut engine = shortcut_engine();
    engine.process_key(&press('m'));
    let result = engine.process_key(&press('a'));

    let shown = shown(&result);
    assert!(!shown.iter().any(|t| t.contains('@')), "shown: {shown:?}");
    assert_eq!(engine.preedit().unwrap().text(), "ま");
}

#[test]
fn a_tail_behind_a_reading_is_not_a_shortcut() {
    // 「まさひろ」 typed as far as `masahirom` must not offer the `m`
    // entry: there is a base reading the exact hit would ignore.
    let mut engine = shortcut_engine();
    type_keys(&mut engine, "masahirom");

    let result = engine.process_key(&press_key(Keysym::SPACE));
    let shown = shown(&result);
    assert!(!shown.iter().any(|t| t.contains('@')), "shown: {shown:?}");
}

#[test]
fn an_unknown_keystroke_adds_nothing() {
    // No entry read `k`, so typing one behaves exactly as before.
    let mut engine = shortcut_engine();
    let result = engine.process_key(&press('k'));
    assert!(
        !shown(&result).iter().any(|t| t.contains('@')),
        "shown: {:?}",
        shown(&result)
    );
    assert_eq!(engine.preedit().unwrap().text(), "k");
}
