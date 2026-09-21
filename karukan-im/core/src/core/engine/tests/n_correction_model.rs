//! What the repair does with a real model behind it.
//!
//! `#[ignore]`d: these load jinen-v2-small (downloaded on first run) and
//! are the only place the replace/offer/drop verdict can be exercised at
//! all — every other test runs without a converter, where a repair is
//! only ever offered. Run them after touching the thresholds:
//!
//! ```text
//! cargo test -p karukan-im --lib n_correction_model --release -- --ignored --nocapture
//! ```

use super::*;
use crate::config::settings::Settings;
use karukan_engine::{KanaKanjiConverter, ModelSource};

fn source() -> ModelSource {
    ModelSource::HuggingFace {
        repo: "togatogah/jinen-v2-small.gguf".to_string(),
        filename: "jinen-v2-small-Q5_K_M.gguf".to_string(),
    }
}

/// Type `keys`, press Space, return the candidate list. `None` when the
/// model could not be loaded, so an offline run skips instead of failing.
fn converted(keys: &str) -> Option<Vec<String>> {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = KanaKanjiConverter::from_source(&source(), "test").ok();
    engine.converters.kanji.as_ref()?;

    type_keys(&mut engine, keys);
    engine.process_key(&press_key(Keysym::SPACE));
    match engine.state() {
        InputState::Conversion { candidates, .. } => Some(
            candidates
                .candidates()
                .iter()
                .map(|c| c.text.clone())
                .collect(),
        ),
        _ => Some(Vec::new()),
    }
}

/// Typing that lost an `n`: the repair is what Space should land on.
#[test]
#[ignore = "loads a model"]
fn a_real_slip_is_replaced() {
    for (keys, expected) in [
        ("sinyabasuninoritai", "深夜バスに乗りたい"),
        ("konnichiha", "こんにちは"),
        ("kaninahouhou", "簡易な方法"),
    ] {
        let Some(got) = converted(keys) else {
            eprintln!("model unavailable, skipping");
            return;
        };
        assert_eq!(
            got.first().map(String::as_str),
            Some(expected),
            "{keys}: {got:?}"
        );
    }
}

/// Correct typing whose repair happens to convert to something the model
/// likes. Nothing here may be replaced — a false repair silently rewrites
/// what someone typed, which is the failure this feature must not have.
#[test]
#[ignore = "loads a model"]
fn correct_typing_is_left_alone() {
    for (keys, expected) in [
        // 簡易を食べる scores *better* than カニを食べる; only the ratio
        // gate keeps the crab.
        ("kaniwotaberu", "カニを食べる"),
        // んえこがすきです scored better still — the leading-ん guard is
        // what stops this one before it is ever weighed.
        ("nekogasukidesu", "猫が好きです"),
        ("nihongonobenkyou", "日本語の勉強"),
        ("natsuyasuminoyotei", "夏休みの予定"),
        ("konosiryouwoonegaisimasu", "この資料をお願いします"),
    ] {
        let Some(got) = converted(keys) else {
            eprintln!("model unavailable, skipping");
            return;
        };
        assert_eq!(
            got.first().map(String::as_str),
            Some(expected),
            "{keys}: {got:?}"
        );
    }
}

/// A repair the model is far surer of *against* is not worth a slot.
#[test]
#[ignore = "loads a model"]
fn a_repair_the_model_disowns_is_dropped() {
    let Some(got) = converted("nihongonobenkyou") else {
        eprintln!("model unavailable, skipping");
        return;
    };
    assert!(!got.iter().any(|t| t.contains("にほんごんお")), "{got:?}");
}

/// An engine set up the way the frontends set one up: both models, live
/// conversion on, the shipped `num_candidates`. `None` when the models
/// could not be loaded, so an offline run skips instead of failing.
fn shipped_engine() -> Option<InputMethodEngine> {
    let light = ModelSource::HuggingFace {
        repo: "togatogah/jinen-v2-xsmall.gguf".to_string(),
        filename: "jinen-v2-xsmall-Q5_K_M.gguf".to_string(),
    };
    let mut engine =
        InputMethodEngine::with_config(EngineConfig::from_settings(&Settings::default()));
    engine.converters.kanji = KanaKanjiConverter::from_source(&source(), "main").ok();
    engine.converters.kanji.as_ref()?;
    engine.converters.light_kanji = KanaKanjiConverter::from_source(&light, "light").ok();
    Some(engine)
}

/// What live conversion shows while `keys` is being typed — before any
/// Space, which is what the user is actually looking at.
fn live(keys: &str) -> Option<String> {
    let mut engine = shipped_engine()?;
    type_keys(&mut engine, keys);
    Some(engine.preedit()?.text().to_string())
}

/// The repair has to reach the live display, not just the candidate
/// list: live conversion's whole job is to show what Enter will commit,
/// so a correction that only appeared on Space would show the wrong
/// thing for as long as the word was being typed.
#[test]
#[ignore = "loads a model"]
fn live_conversion_shows_the_repair() {
    for (keys, expected) in [
        ("sinyabasuninoritai", "深夜バスに乗りたい"),
        ("konnichiha", "こんにちは"),
    ] {
        let Some(got) = live(keys) else {
            eprintln!("model unavailable, skipping");
            return;
        };
        assert_eq!(got, expected, "{keys}");
    }
}

/// …and must leave correct typing alone there too.
#[test]
#[ignore = "loads a model"]
fn live_conversion_leaves_correct_typing_alone() {
    for (keys, expected) in [
        ("kaniwotaberu", "カニを食べる"),
        ("nekogasukidesu", "猫が好きです"),
        ("natsuyasuminoyotei", "夏休みの予定"),
    ] {
        let Some(got) = live(keys) else {
            eprintln!("model unavailable, skipping");
            return;
        };
        assert_eq!(got, expected, "{keys}");
    }
}

/// Mid-word, with a romaji tail still in hand, the keystrokes say more
/// than the reading does: `sinyabasunin` reads しにゃばすに with an `n`
/// left over. A repair cut from them would be weighed against a
/// conversion of something else, so none is cut at all.
#[test]
#[ignore = "loads a model"]
fn a_live_romaji_tail_suppresses_the_repair() {
    let Some(mut engine) = shipped_engine() else {
        eprintln!("model unavailable, skipping");
        return;
    };
    type_keys(&mut engine, "sinyabasunin");
    assert!(!engine.input_buf.pending().is_empty());
    assert!(engine.live.correction.is_none());
}
