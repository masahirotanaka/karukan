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
